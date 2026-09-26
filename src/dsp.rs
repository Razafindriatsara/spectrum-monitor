//! Spektrumschätzung nach Welch: Hann-Fenster, FFT, Mittelung über Segmente.

use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::f32::consts::PI;
use std::sync::Arc;

/// Wie mehrere FFT-Segmente eines Frames zu einem Wert pro Bin werden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Detector {
    /// Mittelwert der Leistung: ruhiges Rauschen, kurze Bursts wirken schwächer.
    Avg,
    /// Maximum: macht kurze Bursts sichtbar, Rauschen wirkt höher.
    Peak,
}

pub struct SpectrumEstimator {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    norm: f32,
    buf: Vec<Complex32>,
    scratch: Vec<Complex32>,
    acc: Vec<f32>,
}

impl SpectrumEstimator {
    pub fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && n >= 16, "FFT-Größe muss eine Zweierpotenz ≥ 16 sein");
        let fft = FftPlanner::new().plan_fft_forward(n);
        let window: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos()).collect();
        // Normierung so, dass ein komplexer Ton mit Amplitude 1 genau 0 dBFS ergibt.
        let sum: f32 = window.iter().sum();
        Self {
            scratch: vec![Complex32::ZERO; fft.get_inplace_scratch_len()],
            fft,
            window,
            norm: 1.0 / (sum * sum),
            buf: vec![Complex32::ZERO; n],
            acc: vec![0.0; n],
        }
    }

    pub fn size(&self) -> usize {
        self.window.len()
    }

    /// Äquivalente Rauschbandbreite des Hann-Fensters (1,5 Bins) in Hz.
    pub fn rbw(&self, sample_rate: f64) -> f64 {
        1.5 * sample_rate / self.size() as f64
    }

    /// Liefert das Spektrum in dBFS, zentriert (negative Frequenzen links).
    /// Samples am Ende, die kein volles Segment mehr ergeben, werden verworfen.
    pub fn process(&mut self, samples: &[Complex32], detector: Detector) -> Vec<f32> {
        let n = self.size();
        let segments = samples.len() / n;
        assert!(segments > 0, "mindestens {n} Samples nötig");
        self.acc.fill(0.0);

        for seg in samples.chunks_exact(n) {
            for ((b, s), w) in self.buf.iter_mut().zip(seg).zip(&self.window) {
                *b = s * w;
            }
            self.fft.process_with_scratch(&mut self.buf, &mut self.scratch);
            for (a, b) in self.acc.iter_mut().zip(&self.buf) {
                let p = b.norm_sqr() * self.norm;
                match detector {
                    Detector::Avg => *a += p,
                    Detector::Peak => *a = a.max(p),
                }
            }
        }
        if detector == Detector::Avg {
            let inv = 1.0 / segments as f32;
            self.acc.iter_mut().for_each(|a| *a *= inv);
        }

        // fftshift: Bin n/2 (höchste negative Frequenz) kommt nach links.
        (0..n).map(|i| 10.0 * (self.acc[(i + n / 2) % n] + 1e-20).log10()).collect()
    }
}

/// Oszillator mit fester Frequenz: dreht einen Zeiger pro Sample um einen
/// festen Winkel. Eine komplexe Multiplikation statt sin und cos pro Sample.
#[derive(Clone)]
pub struct Oscillator {
    rot: Complex32,
    step: Complex32,
}

impl Oscillator {
    pub fn new(freq_hz: f64, sample_rate: f64) -> Self {
        let (sin, cos) = (std::f64::consts::TAU * freq_hz / sample_rate).sin_cos();
        Self { rot: Complex32::new(1.0, 0.0), step: Complex32::new(cos as f32, sin as f32) }
    }

    pub fn sample(&mut self) -> Complex32 {
        let v = self.rot;
        self.rot *= self.step;
        // Ein Newton-Schritt hält den Betrag bei 1. Ohne ihn wächst er durch
        // Rundungsfehler, und das Nachnormieren in Abständen wäre eine
        // periodische Amplitudenmodulation, die der Klassifikator sieht.
        self.rot *= 1.5 - 0.5 * self.rot.norm_sqr();
        v
    }
}

#[cfg(test)]
mod oscillator_tests {
    use super::*;

    #[test]
    fn betrag_bleibt_eins_und_frequenz_stimmt() {
        let mut osc = Oscillator::new(1_000.0, 48_000.0);
        let v: Vec<Complex32> = (0..480_000).map(|_| osc.sample()).collect();
        assert!(v.iter().all(|z| (z.norm() - 1.0).abs() < 1e-6));
        // Nach 10 s bei 1 kHz: genau 10 000 Umläufe, also wieder bei 1.
        assert!((osc.sample() - Complex32::new(1.0, 0.0)).norm() < 1e-2);
    }
}

/// Abtastrate nach dem [`Downconverter`]: breit genug für Schmalbandsignale
/// bis etwa ±20 kHz, und 9600 Bit/s ergeben genau fünf Samples pro Bit.
pub const NARROW_RATE: f64 = 48_000.0;
/// Zwischenrate nach der ersten Dezimierstufe.
const STAGE1_RATE: f64 = 240_000.0;

/// Schneidet ein Schmalbandsignal aus dem breitbandigen Strom: mischt es nach
/// 0 Hz und dezimiert zweistufig auf [`NARROW_RATE`], zum Beispiel
/// 2,4 MHz → 240 kHz → 48 kHz. Der Zustand trägt über Blockgrenzen hinweg.
pub struct Downconverter {
    mixer: Oscillator,
    stage1: Decimator,
    stage2: Decimator,
    buf: Vec<Complex32>,
}

impl Downconverter {
    /// `offset_hz` ist die Frequenz des Signals relativ zur Bandmitte,
    /// `cutoff_hz` die Grenzfrequenz des Kanalfilters (höchstens 20 kHz).
    /// Die Abtastrate muss ein ganzzahliges Vielfaches von 240 kHz sein.
    pub fn new(offset_hz: f64, sample_rate: f64, cutoff_hz: f64) -> Self {
        let decim = (sample_rate / STAGE1_RATE).round() as usize;
        assert!(
            decim >= 1 && (decim as f64 * STAGE1_RATE - sample_rate).abs() < 1.0,
            "Abtastrate muss ein Vielfaches von 240 kHz sein, ist {sample_rate} Hz"
        );
        Self {
            mixer: Oscillator::new(-offset_hz, sample_rate),
            // Stufe 1 muss nur verhindern, dass etwas in die Durchlassbreite von
            // Stufe 2 faltet; Stufe 2 ist das eigentliche Kanalfilter.
            stage1: Decimator::new(lowpass(64, 60_000.0 / sample_rate), decim),
            stage2: Decimator::new(lowpass(96, cutoff_hz / STAGE1_RATE), (STAGE1_RATE / NARROW_RATE) as usize),
            buf: Vec::new(),
        }
    }

    /// Hängt die schmalbandigen Samples zu `samples` an `out` an.
    pub fn process(&mut self, samples: &[Complex32], out: &mut Vec<Complex32>) {
        self.buf.clear();
        for &s in samples {
            if let Some(y) = self.stage1.push(s * self.mixer.sample()) {
                self.buf.push(y);
            }
        }
        out.extend(self.buf.iter().filter_map(|&y| self.stage2.push(y)));
    }
}

/// FIR-Tiefpass mit Dezimation: berechnet nur jedes `factor`-te Ausgangssample.
struct Decimator {
    taps: Vec<f32>,
    /// Ringpuffer doppelter Länge, damit das Fenster immer zusammenhängend ist.
    hist: Vec<Complex32>,
    pos: usize,
    factor: usize,
    phase: usize,
}

impl Decimator {
    fn new(taps: Vec<f32>, factor: usize) -> Self {
        Self { hist: vec![Complex32::ZERO; 2 * taps.len()], taps, pos: 0, factor, phase: 0 }
    }

    fn push(&mut self, x: Complex32) -> Option<Complex32> {
        let n = self.taps.len();
        self.hist[self.pos] = x;
        self.hist[self.pos + n] = x;
        self.pos = (self.pos + 1) % n;
        self.phase += 1;
        if self.phase < self.factor {
            return None;
        }
        self.phase = 0;
        let window = &self.hist[self.pos..self.pos + n];
        Some(window.iter().zip(&self.taps).map(|(x, t)| x * t).sum())
    }
}

/// Gefensterter Sinc-Tiefpass (Blackman), Grenzfrequenz relativ zur Abtastrate.
fn lowpass(len: usize, cutoff: f64) -> Vec<f32> {
    use std::f64::consts::{PI, TAU};
    let m = (len - 1) as f64;
    let taps: Vec<f64> = (0..len)
        .map(|i| {
            let x = i as f64 - m / 2.0;
            let sinc = if x == 0.0 { 2.0 * cutoff } else { (TAU * cutoff * x).sin() / (PI * x) };
            let w = 0.42 - 0.5 * (TAU * i as f64 / m).cos() + 0.08 * (2.0 * TAU * i as f64 / m).cos();
            sinc * w
        })
        .collect();
    let sum: f64 = taps.iter().sum();
    taps.iter().map(|t| (t / sum) as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, len: usize, bin: f32, amp: f32) -> Vec<Complex32> {
        (0..len).map(|i| Complex32::from_polar(amp, 2.0 * PI * bin * i as f32 / n as f32)).collect()
    }

    fn argmax(v: &[f32]) -> usize {
        v.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0
    }

    #[test]
    fn positiver_ton_landet_rechts_der_mitte() {
        let n = 1024;
        let spec = SpectrumEstimator::new(n).process(&tone(n, 4 * n, 100.0, 1.0), Detector::Avg);
        assert_eq!(argmax(&spec), n / 2 + 100);
    }

    #[test]
    fn negativer_ton_landet_links_der_mitte() {
        let n = 1024;
        let spec = SpectrumEstimator::new(n).process(&tone(n, n, -200.0, 1.0), Detector::Avg);
        assert_eq!(argmax(&spec), n / 2 - 200);
    }

    #[test]
    fn pegel_ist_in_dbfs_kalibriert() {
        let n = 2048;
        let spec = SpectrumEstimator::new(n).process(&tone(n, n, 37.0, 0.1), Detector::Avg);
        let peak = spec[n / 2 + 37];
        assert!((peak - -20.0).abs() < 0.05, "erwartet −20 dBFS, gemessen {peak}");
    }

    #[test]
    fn peak_detektor_haelt_kurzen_burst() {
        let n = 256;
        let mut s = vec![Complex32::ZERO; 8 * n];
        s[..n].copy_from_slice(&tone(n, n, 10.0, 1.0));
        let mut est = SpectrumEstimator::new(n);
        let avg = est.process(&s, Detector::Avg)[n / 2 + 10];
        let peak = est.process(&s, Detector::Peak)[n / 2 + 10];
        // Ein Segment von acht: Mittelung kostet 10·log10(8) ≈ 9 dB.
        assert!((peak - avg - 9.03).abs() < 0.05);
    }
}
