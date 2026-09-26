//! Signalgeneratoren für den Simulator und für die Trainingsdaten des
//! Klassifikators. Jeder Generator liefert komplexes Basisband um 0 Hz mit
//! einer mittleren Leistung von etwa 1; ins Band verschoben wird erst beim Mischen.

use crate::dsp::Oscillator;
use rand::RngExt;
use rand::rngs::StdRng;
use rustfft::num_complex::Complex32;
use std::f64::consts::{PI, TAU};

/// Numerisch gesteuerter Oszillator für veränderliche Frequenzen: hält die
/// Phase über Blockgrenzen hinweg. Für feste Frequenzen ist [`Oscillator`] schneller.
#[derive(Default)]
pub struct Nco {
    phase: f64,
}

impl Nco {
    pub fn next(&mut self, freq: f64, fs: f64) -> Complex32 {
        self.phase = (self.phase + TAU * freq / fs) % TAU;
        let (sin, cos) = self.phase.sin_cos();
        Complex32::new(cos as f32, sin as f32)
    }
}

/// Modulationsarten, die der Simulator erzeugen kann.
pub enum Modulation {
    /// Unmodulierter Träger, z. B. eine Bake.
    Carrier,
    /// Amplitudenmodulation mit einem Ton. Der Tonoszillator entsteht beim
    /// ersten Sample, wenn die Abtastrate bekannt ist.
    Am {
        tone: Option<Oscillator>,
        tone_hz: f64,
        depth: f32,
    },
    /// Frequenzmodulation mit einem Ton.
    Fm {
        phase: f32,
        tone: Option<Oscillator>,
        tone_hz: f64,
        deviation_hz: f64,
    },
    Qpsk(Qpsk),
    /// Burst mit vorgegebenem Frequenzverlauf, z. B. GMSK aus [`crate::ais::modulate`].
    FreqBurst {
        freq: Vec<f32>,
        delay: usize,
        pos: usize,
        phase: f64,
        ramp: usize,
    },
}

impl Modulation {
    pub fn am(tone_hz: f64, depth: f32) -> Self {
        Self::Am { tone: None, tone_hz, depth }
    }

    pub fn fm(tone_hz: f64, deviation_hz: f64) -> Self {
        Self::Fm { phase: 0.0, tone: None, tone_hz, deviation_hz }
    }

    /// Burst, der nach `delay` Samples beginnt; die Flanken steigen über `ramp` Samples an.
    pub fn burst(freq: Vec<f32>, delay: usize, ramp: usize) -> Self {
        Self::FreqBurst { freq, delay, pos: 0, phase: 0.0, ramp: ramp.max(1) }
    }

    pub fn next(&mut self, fs: f64, rng: &mut StdRng) -> Complex32 {
        match self {
            Modulation::Carrier => Complex32::new(1.0, 0.0),
            Modulation::Am { tone, tone_hz, depth } => {
                let m = tone.get_or_insert_with(|| Oscillator::new(*tone_hz, fs)).sample().re;
                Complex32::new((1.0 + *depth * m) / (1.0 + *depth * *depth / 2.0).sqrt(), 0.0)
            }
            Modulation::Fm { phase, tone, tone_hz, deviation_hz } => {
                let m = tone.get_or_insert_with(|| Oscillator::new(*tone_hz, fs)).sample().re;
                *phase = (*phase + (TAU * *deviation_hz / fs) as f32 * m) % std::f32::consts::TAU;
                Complex32::from_polar(1.0, *phase)
            }
            Modulation::Qpsk(q) => q.next(fs, rng),
            Modulation::FreqBurst { freq, delay, pos, phase, ramp } => {
                if *delay > 0 {
                    *delay -= 1;
                    return Complex32::ZERO;
                }
                let Some(&f) = freq.get(*pos) else { return Complex32::ZERO };
                *phase = (*phase + TAU * f64::from(f) / fs) % TAU;
                let edge = (*pos).min(freq.len() - 1 - *pos).min(*ramp);
                *pos += 1;
                Complex32::from_polar(edge as f32 / *ramp as f32, *phase as f32)
            }
        }
    }
}

/// QPSK mit Raised-Cosine-Pulsformung. Der Puls liegt als Tabelle vor, damit
/// auch 2,4 MHz Abtastrate in Echtzeit gehen.
pub struct Qpsk {
    symbol_rate: f64,
    /// Zeit seit dem mittleren Symbol, in Symboldauern.
    t: f64,
    symbols: [Complex32; 2 * SPAN + 1],
    pulse: Vec<f32>,
}

/// Der Puls wird auf ±SPAN Symbole abgeschnitten.
const SPAN: usize = 3;
const STEPS: usize = 64;

impl Qpsk {
    pub fn new(symbol_rate: f64, rolloff: f64) -> Self {
        // Raised Cosine auf [-SPAN-1, SPAN+1] mit STEPS Stützstellen pro Symbol.
        let n = 2 * (SPAN + 1) * STEPS + 1;
        let pulse = (0..n)
            .map(|i| {
                let t = i as f64 / STEPS as f64 - (SPAN + 1) as f64;
                let sinc = if t == 0.0 { 1.0 } else { (PI * t).sin() / (PI * t) };
                let d = 1.0 - (2.0 * rolloff * t).powi(2);
                let rc = if d.abs() < 1e-9 { PI / 4.0 } else { (PI * rolloff * t).cos() / d };
                (sinc * rc) as f32
            })
            .collect();
        Self { symbol_rate, t: 0.0, symbols: [Complex32::ZERO; 2 * SPAN + 1], pulse }
    }

    fn next(&mut self, fs: f64, rng: &mut StdRng) -> Complex32 {
        let mut v = Complex32::ZERO;
        for (j, s) in self.symbols.iter().enumerate() {
            let x = (self.t - j as f64 + SPAN as f64 + (SPAN + 1) as f64) * STEPS as f64;
            let (i, frac) = (x.floor() as usize, (x - x.floor()) as f32);
            if let (Some(a), Some(b)) = (self.pulse.get(i), self.pulse.get(i + 1)) {
                v += s * (a + (b - a) * frac);
            }
        }
        self.t += self.symbol_rate / fs;
        while self.t >= 1.0 {
            self.t -= 1.0;
            self.symbols.rotate_left(1);
            let (i, q) = (rng.random_bool(0.5), rng.random_bool(0.5));
            let a = std::f32::consts::FRAC_1_SQRT_2;
            self.symbols[2 * SPAN] = Complex32::new(if i { a } else { -a }, if q { a } else { -a });
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    fn power(m: &mut Modulation, n: usize) -> f32 {
        let mut rng = StdRng::seed_from_u64(1);
        let fs = 2.4e6;
        // Einschwingen der Pulsformung überspringen.
        (0..1000).for_each(|_| _ = m.next(fs, &mut rng));
        (0..n).map(|_| m.next(fs, &mut rng).norm_sqr()).sum::<f32>() / n as f32
    }

    #[test]
    fn generatoren_haben_etwa_leistung_eins() {
        for (name, mut m) in [
            ("Träger", Modulation::Carrier),
            ("AM", Modulation::am(1_000.0, 0.8)),
            ("FM", Modulation::fm(1_000.0, 5_000.0)),
            ("QPSK", Modulation::Qpsk(Qpsk::new(9_600.0, 0.35))),
        ] {
            let p = power(&mut m, 240_000);
            assert!((0.8..1.2).contains(&p), "{name}: Leistung {p}");
        }
    }

    #[test]
    fn burst_beginnt_nach_verzoegerung_und_endet() {
        let mut rng = StdRng::seed_from_u64(1);
        let mut m = Modulation::burst(vec![0.0; 100], 10, 5);
        let v: Vec<f32> = (0..130).map(|_| m.next(1e6, &mut rng).norm()).collect();
        assert!(v[..10].iter().all(|&a| a == 0.0));
        assert!((v[60] - 1.0).abs() < 1e-6);
        assert!(v[110..].iter().all(|&a| a == 0.0));
    }
}
