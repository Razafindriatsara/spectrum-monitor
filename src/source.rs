//! IQ-Quellen. Alles, was komplexe Basisband-Samples liefert, implementiert
//! [`IqSource`]. Heute gibt es den Simulator, später kommt ein RTL-SDR dazu,
//! ohne dass sich DSP oder Server ändern müssen.

use crate::ais::{self, Channel, fleet::Fleet, gmsk::BAUD};
use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use rand_distr::{Distribution, Normal};
use rustfft::num_complex::Complex32;
use std::collections::VecDeque;
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

/// Ein AIS-Kanal: sendet fertig modulierte Bursts nacheinander, mit mindestens
/// einem Zeitschlitz Abstand, damit sich Aussendungen nicht überlagern.
struct AisChannel {
    offset: f64,
    nco: Nco,
    queue: VecDeque<Burst>,
    current: Option<Burst>,
    pos: usize,
    guard: usize,
}

struct Burst {
    freq: Vec<f32>,
    amplitude: f32,
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
    fleet: Fleet,
    ais: [AisChannel; 2],
}

const HOP_S: f64 = 0.05;
/// Ein AIS-Zeitschlitz dauert 60 s / 2250.
const SLOT_S: f64 = 60.0 / 2250.0;

impl SimulatedSource {
    /// Simuliert 2,4 MHz um 162 MHz, also das AIS-Band mit beiden Kanälen
    /// (161,975 und 162,025 MHz), dort den Schiffsverkehr der Kieler Förde,
    /// plus einige weitere Sender.
    pub fn new(seed: u64) -> Self {
        let fs = 2.4e6;
        let fc = 162.0e6;
        let mut rng = StdRng::seed_from_u64(seed);
        let ais = [Channel::A, Channel::B].map(|ch| AisChannel {
            offset: ch.freq_hz() - fc,
            nco: Nco { phase: 0.0 },
            queue: VecDeque::new(),
            current: None,
            pos: 0,
            guard: 0,
        });
        Self {
            fs,
            fc,
            fleet: Fleet::kiel(&mut rng),
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

impl AisChannel {
    fn next(&mut self, fs: f64) -> Complex32 {
        if self.current.is_none() {
            if self.guard > 0 {
                self.guard -= 1;
                return Complex32::ZERO;
            }
            self.current = self.queue.pop_front();
            self.pos = 0;
        }
        let Some(burst) = &self.current else { return Complex32::ZERO };
        // Weiche Flanken über eine Bitdauer, wie beim Sender vorgeschrieben.
        let ramp = (fs / BAUD) as usize;
        let edge = self.pos.min(burst.freq.len() - 1 - self.pos).min(ramp);
        let v = self.nco.next(self.offset + f64::from(burst.freq[self.pos]), fs)
            * (burst.amplitude * edge as f32 / ramp as f32);
        self.pos += 1;
        if self.pos == burst.freq.len() {
            self.current = None;
            self.guard = (SLOT_S * fs) as usize;
        }
        v
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
        let sps = (fs / BAUD).round() as usize;
        for (msg, ch, amplitude) in self.fleet.advance(buf.len() as f64 / fs, &mut self.rng) {
            let freq = ais::modulate(&msg.encode(), sps);
            self.ais[ch as usize].queue.push_back(Burst { freq, amplitude });
        }
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
                v += ch.next(fs);
            }

            v += Complex32::new(self.noise.sample(&mut self.rng), self.noise.sample(&mut self.rng));
            *s = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::{AisMessage, AisReceiver};
    use std::collections::BTreeSet;

    #[test]
    fn empfaenger_hoert_die_ganze_flotte() {
        let mut src = SimulatedSource::new(7);
        let mut rx = AisReceiver::new(src.sample_rate(), src.center_freq());
        let mut buf = vec![Complex32::ZERO; 96_000];
        let mut names = BTreeSet::new();
        // 12 s Signalzeit: jedes Schiff sendet in den ersten 5 s seine Stammdaten.
        for _ in 0..300 {
            src.read(&mut buf);
            for f in rx.process(&buf) {
                if let Ok(AisMessage::Static(s)) = AisMessage::decode(&f.payload) {
                    names.insert(s.name);
                }
            }
        }
        let expected = ["BALTIC AMBER", "LABOE", "MOEWE", "NORDLICHT", "SCHWENTINE", "SPROTTE"];
        assert_eq!(names, expected.map(String::from).into());
    }
}
