//! IQ-Quellen. Alles, was komplexe Basisband-Samples liefert, implementiert
//! [`IqSource`]. Heute gibt es den Simulator, später kommt ein RTL-SDR dazu,
//! ohne dass sich DSP oder Server ändern müssen.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use rand_distr::{Distribution, Normal};
use rustfft::num_complex::Complex32;
use std::f64::consts::TAU;

pub trait IqSource: Send {
    /// Abtastrate in Samples pro Sekunde.
    fn sample_rate(&self) -> f64;
    /// Mittenfrequenz in Hz (entspricht 0 Hz im Basisband).
    fn center_freq(&self) -> f64;
    /// Füllt `buf` mit den nächsten Samples, lückenlos in Echtzeit-Signalzeit.
    fn read(&mut self, buf: &mut [Complex32]);
}

/// Numerisch gesteuerter Oszillator: hält die Phase über Blockgrenzen hinweg.
struct Nco {
    phase: f64,
}

impl Nco {
    fn next(&mut self, freq: f64, fs: f64) -> Complex32 {
        self.phase = (self.phase + TAU * freq / fs) % TAU;
        Complex32::new(self.phase.cos() as f32, self.phase.sin() as f32)
    }
}

/// FSK-Bursts nach Art von AIS: 9600 Baud, ±2,4 kHz Hub, 26,7 ms lang.
struct BurstChannel {
    offset: f64,
    nco: Nco,
    remaining: usize,
    until_next: usize,
    bit_left: usize,
    bit: bool,
}

/// Frequenzspringer: wechselt alle 50 ms auf eine zufällige Frequenz.
struct Hopper {
    nco: Nco,
    freq: f64,
    left: usize,
}

pub struct SimulatedSource {
    fs: f64,
    fc: f64,
    rng: StdRng,
    noise: Normal<f32>,
    carrier: Nco,
    fm: Nco,
    fm_mod: Nco,
    hopper: Hopper,
    ais: [BurstChannel; 2],
}

const BAUD: f64 = 9600.0;
const FSK_DEV: f64 = 2400.0;
const BURST_S: f64 = 0.02667;
const HOP_S: f64 = 0.05;

impl SimulatedSource {
    /// Simuliert 2,4 MHz um 162 MHz, also das AIS-Band mit beiden Kanälen
    /// (161,975 und 162,025 MHz) plus einigen weiteren Sendern.
    pub fn new(seed: u64) -> Self {
        let fs = 2.4e6;
        let mut rng = StdRng::seed_from_u64(seed);
        let first_gap = |rng: &mut StdRng| (rng.random_range(0.2..1.5) * fs) as usize;
        let ais = [BurstChannel::new(-25_000.0, first_gap(&mut rng)), BurstChannel::new(25_000.0, first_gap(&mut rng))];
        Self {
            fs,
            fc: 162.0e6,
            noise: Normal::new(0.0, 0.003).expect("gültige Standardabweichung"),
            carrier: Nco { phase: 0.0 },
            fm: Nco { phase: 0.0 },
            fm_mod: Nco { phase: 0.0 },
            hopper: Hopper { nco: Nco { phase: 0.0 }, freq: 700_000.0, left: 0 },
            ais,
            rng,
        }
    }
}

impl BurstChannel {
    fn new(offset: f64, until_next: usize) -> Self {
        Self { offset, nco: Nco { phase: 0.0 }, remaining: 0, until_next, bit_left: 0, bit: false }
    }

    fn next(&mut self, fs: f64, rng: &mut StdRng) -> Complex32 {
        if self.remaining == 0 {
            if self.until_next > 0 {
                self.until_next -= 1;
                return Complex32::ZERO;
            }
            // Neuer Burst beginnt, nächste Pause wird schon ausgewürfelt.
            self.remaining = (BURST_S * fs) as usize;
            self.until_next = (rng.random_range(0.4..2.5) * fs) as usize;
        }
        if self.bit_left == 0 {
            self.bit = rng.random_bool(0.5);
            self.bit_left = (fs / BAUD) as usize;
        }
        self.bit_left -= 1;
        self.remaining -= 1;
        let dev = if self.bit { FSK_DEV } else { -FSK_DEV };
        self.nco.next(self.offset + dev, fs) * 0.08
    }
}

impl IqSource for SimulatedSource {
    fn sample_rate(&self) -> f64 {
        self.fs
    }

    fn center_freq(&self) -> f64 {
        self.fc
    }

    fn read(&mut self, buf: &mut [Complex32]) {
        let fs = self.fs;
        for s in buf.iter_mut() {
            // Dauerträger, z. B. eine Bake.
            let mut v = self.carrier.next(-600_000.0, fs) * 0.05;

            // Schmalband-FM: 1-kHz-Ton mit 5 kHz Hub.
            let m = self.fm_mod.next(1_000.0, fs).re as f64;
            v += self.fm.next(-300_000.0 + 5_000.0 * m, fs) * 0.02;

            // Frequenzspringer im oberen Teil des Spektrums.
            if self.hopper.left == 0 {
                self.hopper.freq = self.rng.random_range(250_000.0..1_100_000.0);
                self.hopper.left = (HOP_S * fs) as usize;
            }
            self.hopper.left -= 1;
            v += self.hopper.nco.next(self.hopper.freq, fs) * 0.01;

            for ch in &mut self.ais {
                v += ch.next(fs, &mut self.rng);
            }

            v += Complex32::new(self.noise.sample(&mut self.rng), self.noise.sample(&mut self.rng));
            *s = v;
        }
    }
}
