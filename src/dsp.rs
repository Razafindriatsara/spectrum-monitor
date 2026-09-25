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
