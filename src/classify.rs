//! Modulationsklassifikation mit einem kleinen 1D-CNN im ONNX-Format.
//!
//! Der Weg ist für Training und Betrieb derselbe, damit das Modell zur Laufzeit
//! genau das sieht, womit es trainiert wurde:
//!
//! 1. [`extract`]: das Signal aus dem breitbandigen IQ-Block mischen, auf 48 kHz
//!    dezimieren und das Fenster mit der meisten Energie nehmen. So landen
//!    auch kurze Bursts im Fenster.
//! 2. [`features`]: pro Sample Betrag und Momentanfrequenz, dazu das
//!    Betragsspektrum des Fensters. Alle drei hängen weder von der Trägerphase
//!    noch vom Pegel ab: Der Betrag ist auf die mittlere Leistung normiert, die
//!    Frequenz so skaliert, dass typische Hübe um 1 liegen, das Spektrum auf
//!    Mittelwert 0 und Streuung 1 gebracht. I und Q selbst fehlen bewusst: Sie
//!    tragen vor allem die zufällige Phase, und ein Träger genau bei 0 Hz sähe
//!    darin ganz anders aus als einer knapp daneben. Das Spektrum zeigt, was im
//!    Zeitbereich nur über lange Strecken sichtbar wird, etwa Seitenbänder.
//! 3. [`Classifier`]: ONNX-Inferenz mit tract, reines Rust ohne native Bibliothek.
//!
//! [`synthesize`] erzeugt Trainingsbeispiele mit denselben Generatoren wie der
//! Simulator; trainiert wird in `src/bin/train`.

use crate::ais;
use crate::dsp::{Downconverter, Oscillator};
use crate::signals::{Modulation, Qpsk};
use rand::RngExt;
use rand::rngs::StdRng;
use rand_distr::{Distribution, Normal};
use rustfft::num_complex::Complex32;
use std::io::Cursor;
use tract_onnx::prelude::*;

/// Samples pro Fenster bei 48 kHz, also gut 21 ms.
pub const WINDOW: usize = 1024;
/// Merkmalskanäle: Betrag, Momentanfrequenz, Spektrum.
pub const CHANNELS: usize = 3;
/// Grenzfrequenz des Ausschnitts; breit genug für alle Klassen.
const CUTOFF_HZ: f64 = 16_000.0;
/// Die ersten Samples nach dem Filter sind noch Einschwingen.
const SETTLE: usize = 32;
/// Diese Frequenzablage ergibt den Merkmalswert 1.
const FREQ_SCALE_HZ: f32 = 5_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    /// Fehlalarm des Detektors: nichts als Rauschen im Fenster.
    Noise,
    Carrier,
    Am,
    Fm,
    Gmsk,
    Qpsk,
}

impl Class {
    /// Reihenfolge der Modellausgänge.
    pub const ALL: [Class; 6] = [Class::Noise, Class::Carrier, Class::Am, Class::Fm, Class::Gmsk, Class::Qpsk];

    /// Position in den Modellausgängen.
    pub fn index(self) -> usize {
        Class::ALL.iter().position(|&c| c == self).expect("jede Klasse steht in ALL")
    }

    pub fn label(self) -> &'static str {
        match self {
            Class::Noise => "Rauschen",
            Class::Carrier => "Träger",
            Class::Am => "AM",
            Class::Fm => "FM",
            Class::Gmsk => "GMSK",
            Class::Qpsk => "QPSK",
        }
    }
}

/// Schneidet das Signal bei `offset_hz` (relativ zur Bandmitte) aus und liefert
/// das energiereichste Fenster mit [`WINDOW`] Samples bei 48 kHz. `None`, wenn
/// der Block dafür zu kurz ist.
pub fn extract(block: &[Complex32], sample_rate: f64, offset_hz: f64) -> Option<Vec<Complex32>> {
    let mut down = Downconverter::new(offset_hz, sample_rate, CUTOFF_HZ);
    let mut narrow = Vec::new();
    down.process(block, &mut narrow);
    let x = narrow.get(SETTLE..)?;
    if x.len() < WINDOW {
        return None;
    }
    let mut prefix = vec![0.0f64; x.len() + 1];
    for (i, v) in x.iter().enumerate() {
        prefix[i + 1] = prefix[i] + f64::from(v.norm_sqr());
    }
    let start = (0..=x.len() - WINDOW)
        .step_by(32)
        .max_by(|&a, &b| (prefix[a + WINDOW] - prefix[a]).total_cmp(&(prefix[b + WINDOW] - prefix[b])))
        .expect("mindestens ein Fenster");
    Some(x[start..start + WINDOW].to_vec())
}

/// Merkmale eines Fensters, kanalweise: erst der Betrag, dann die
/// Momentanfrequenz in Vielfachen von 5 kHz und zuletzt das Spektrum in dB,
/// auf Mittelwert 0 und Standardabweichung 1 gebracht.
pub fn features(x: &[Complex32]) -> Vec<f32> {
    let hz_per_rad = (crate::dsp::NARROW_RATE / std::f64::consts::TAU) as f32;
    let rms = (x.iter().map(|v| v.norm_sqr()).sum::<f32>() / x.len() as f32).sqrt().max(1e-12);
    let mut out = Vec::with_capacity(CHANNELS * x.len());
    out.extend(x.iter().map(|v| v.norm() / rms));
    out.push(0.0);
    out.extend(x.windows(2).map(|w| (w[1] * w[0].conj()).arg() * hz_per_rad / FREQ_SCALE_HZ));
    out.extend(spectrum(x));
    out
}

/// Hann-gefenstertes Betragsspektrum in dB, negative Frequenzen links, normiert.
fn spectrum(x: &[Complex32]) -> Vec<f32> {
    let n = x.len();
    let mut buf: Vec<Complex32> = x
        .iter()
        .enumerate()
        .map(|(i, v)| v * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()))
        .collect();
    rustfft::FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let db: Vec<f32> = (0..n).map(|i| 10.0 * (buf[(i + n / 2) % n].norm_sqr() + 1e-12).log10()).collect();
    let mean = db.iter().sum::<f32>() / n as f32;
    let std = (db.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n as f32).sqrt().max(1e-6);
    db.iter().map(|v| (v - mean) / std).collect()
}

/// Parameter der Trainingsbeispiele. Die Bereiche sind breiter als im
/// Simulator, damit das Modell nicht nur dessen feste Einstellungen lernt.
pub const DATASET_SAMPLE_RATE: f64 = 2.4e6;
/// Blocklänge wie im Betrieb bei 25 Spektren pro Sekunde.
pub const DATASET_BLOCK: usize = 96_000;

/// Erzeugt ein Trainingsbeispiel der Klasse `class` und liefert seine Merkmale.
///
/// Zufällig sind: Rauschabstand 0–50 dB im 48-kHz-Kanal, Frequenzfehler der
/// Detektion bis ±1,5 kHz, Trägerphase, und die Parameter der Modulation.
pub fn synthesize(class: Class, rng: &mut StdRng) -> Vec<f32> {
    let fs = DATASET_SAMPLE_RATE;
    let mut modulation = match class {
        Class::Noise | Class::Carrier => Modulation::Carrier,
        Class::Am => Modulation::am(rng.random_range(300.0..3000.0), rng.random_range(0.3..0.95)),
        Class::Fm => Modulation::fm(rng.random_range(300.0..3000.0), rng.random_range(2000.0..6000.0)),
        Class::Qpsk => Modulation::Qpsk(Qpsk::new(rng.random_range(4800.0..16000.0), rng.random_range(0.2..0.5))),
        Class::Gmsk => {
            // Nutzdaten wie Typ 1 oder Typ 5, der Burst liegt irgendwo im Block
            // und darf über dessen Rand hinausragen.
            let bits = if rng.random_bool(0.7) { 168 } else { 424 };
            let payload: Vec<bool> = (0..bits).map(|_| rng.random_bool(0.5)).collect();
            let sps = (fs / ais::gmsk::BAUD).round() as usize;
            let freq = ais::modulate(&payload, sps);
            let min_overlap = 30_000.min(freq.len());
            let start = rng.random_range(-((freq.len() - min_overlap) as i64)..=(DATASET_BLOCK - min_overlap) as i64);
            let skip = (-start).max(0) as usize;
            Modulation::burst(freq[skip..].to_vec(), start.max(0) as usize, sps)
        }
    };
    let amplitude = if class == Class::Noise { 0.0 } else { 1.0 };
    // Rauschen so, dass es im 48-kHz-Ausschnitt den gewünschten Abstand hat.
    // Bis 50 dB: nahe Sender kommen sehr sauber an, auch das muss das Modell kennen.
    let snr = 10f64.powf(rng.random_range(0.0..50.0) / 10.0);
    let sigma = (fs / (2.0 * crate::dsp::NARROW_RATE * snr)).sqrt() as f32;
    let noise = Normal::new(0.0, sigma).expect("gültige Standardabweichung");
    // Meist ein Schätzfehler der Detektion, oft aber auch fast genau getroffen.
    let offset = if rng.random_bool(0.3) { rng.random_range(-50.0..50.0) } else { rng.random_range(-1500.0..1500.0) };
    let phase = Complex32::from_polar(1.0, rng.random_range(0.0..std::f32::consts::TAU));
    let mut shift = Oscillator::new(offset, fs);

    let block: Vec<Complex32> = (0..DATASET_BLOCK)
        .map(|_| {
            let s = modulation.next(fs, rng) * shift.sample() * phase * amplitude;
            s + Complex32::new(noise.sample(rng), noise.sample(rng))
        })
        .collect();
    features(&extract(&block, fs, 0.0).expect("Block ist lang genug"))
}

/// Führt das ONNX-Modell aus. Eingang `[1, 4, 1024]`, Ausgang die
/// Wahrscheinlichkeiten der Klassen in der Reihenfolge von [`Class::ALL`].
pub struct Classifier {
    plan: std::sync::Arc<TypedSimplePlan>,
}

impl Classifier {
    pub fn from_bytes(onnx: &[u8]) -> TractResult<Self> {
        let plan = tract_onnx::onnx()
            .model_for_read(&mut Cursor::new(onnx))?
            .with_input_fact(0, f32::fact([1, CHANNELS, WINDOW]).into())?
            .into_optimized()?
            .into_runnable()?;
        Ok(Self { plan })
    }

    /// Klassenwahrscheinlichkeiten zu den Merkmalen aus [`features`].
    pub fn classify(&self, features: &[f32]) -> TractResult<Vec<f32>> {
        let input = tract_ndarray::Array3::from_shape_vec((1, CHANNELS, WINDOW), features.to_vec())?;
        let out = self.plan.run(tvec!(Tensor::from(input).into()))?;
        let probs = out[0].to_plain_array_view::<f32>()?.iter().copied().collect::<Vec<_>>();
        if probs.len() != Class::ALL.len() {
            return Err(TractError::msg(format!("Modell liefert {} statt {} Klassen", probs.len(), Class::ALL.len())));
        }
        Ok(probs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn ausschnitt_landet_bei_null_hertz() {
        let fs = 2.4e6;
        let mut osc = Oscillator::new(-450_000.0 + 2_000.0, fs);
        let block: Vec<Complex32> = (0..96_000).map(|_| osc.sample()).collect();
        let x = extract(&block, fs, -450_000.0).unwrap();
        // Übrig bleibt ein Ton bei +2 kHz, also 2/5 der Frequenzskala.
        let f = features(&x);
        let inst = &f[WINDOW + 1..2 * WINDOW];
        let mean = inst.iter().sum::<f32>() / inst.len() as f32;
        assert!((mean - 0.4).abs() < 1e-3, "{mean}");
        // Betrag normiert auf 1.
        assert!(f[..WINDOW].iter().all(|a| (a - 1.0).abs() < 0.01));
        // Spektrum: Spitze bei +2 kHz, also 2/48 der Fensterlänge rechts der Mitte.
        let spec = &f[2 * WINDOW..];
        let peak = spec.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(peak, WINDOW / 2 + (2_000.0 / 48_000.0 * WINDOW as f64).round() as usize);
    }

    #[test]
    fn ausschnitt_findet_den_burst() {
        let fs = 2.4e6;
        let mut block = vec![Complex32::ZERO; 96_000];
        // 10 ms Ton ab Sample 60 000, also mitten im Block.
        for s in &mut block[60_000..84_000] {
            *s = Complex32::new(1.0, 0.0);
        }
        let x = extract(&block, fs, 0.0).unwrap();
        let strong = x.iter().filter(|v| v.norm() > 0.5).count();
        // 24 000 Samples bei 2,4 MHz sind 480 Samples bei 48 kHz, alle im Fenster.
        assert!(strong >= 470, "{strong}");
    }

    #[test]
    fn beispiele_haben_die_richtige_form() {
        let mut rng = StdRng::seed_from_u64(1);
        for c in Class::ALL {
            let f = synthesize(c, &mut rng);
            assert_eq!(f.len(), CHANNELS * WINDOW);
            assert!(f.iter().all(|v| v.is_finite()), "{c:?}");
        }
    }
}
