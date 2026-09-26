//! AIS-Empfänger für beide Kanäle. Pro Kanal:
//!
//! 1. [`Downconverter`]: Kanalmitte nach 0 Hz mischen und auf 48 kHz
//!    dezimieren, also fünf Samples pro Bit. Das Kanalfilter trennt A und B.
//! 2. FM-Diskriminator: Phasendifferenz aufeinanderfolgender Samples.
//! 3. Taktrückgewinnung: Nulldurchgänge ziehen den Abtastzeitpunkt in die Bitmitte.
//! 4. NRZI-Dekodierung und HDLC-Rahmensuche mit CRC-Prüfung.

use super::Channel;
use super::frame::Deframer;
use super::gmsk::{BAUD, DEVIATION_HZ};
use crate::dsp::{Downconverter, NARROW_RATE};
use rustfft::num_complex::Complex32;
use std::f64::consts::TAU;

const SPS: f32 = (NARROW_RATE / BAUD) as f32;
/// Grenzfrequenz des Kanalfilters; der Nachbarkanal liegt 50 kHz daneben.
const CHANNEL_CUTOFF_HZ: f64 = 10_000.0;
/// Anteil des Taktfehlers, der pro Nulldurchgang korrigiert wird.
const CLOCK_GAIN: f32 = 0.3;

/// Ein Rahmen mit gültiger CRC.
#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedFrame {
    pub channel: Channel,
    pub payload: Vec<bool>,
}

/// Empfängt beide AIS-Kanäle aus einem breitbandigen IQ-Strom.
pub struct AisReceiver {
    channels: Vec<ChannelReceiver>,
}

impl AisReceiver {
    /// Die Abtastrate muss ein ganzzahliges Vielfaches von 240 kHz sein, und
    /// beide Kanäle müssen im erfassten Band liegen.
    pub fn new(sample_rate: f64, center_freq: f64) -> Self {
        let channels = [Channel::A, Channel::B]
            .into_iter()
            .map(|ch| {
                let offset = ch.freq_hz() - center_freq;
                assert!(offset.abs() < 0.45 * sample_rate, "Kanal {ch:?} liegt außerhalb des Bandes");
                ChannelReceiver::new(ch, offset, sample_rate)
            })
            .collect();
        Self { channels }
    }

    pub fn process(&mut self, samples: &[Complex32]) -> Vec<ReceivedFrame> {
        let mut out = Vec::new();
        for ch in &mut self.channels {
            ch.process(samples, &mut out);
        }
        out
    }
}

struct ChannelReceiver {
    channel: Channel,
    down: Downconverter,
    prev: Complex32,
    disc_scale: f32,
    clock: f32,
    last_sign: bool,
    last_level: bool,
    deframer: Deframer,
    // Zwischenpuffer, damit pro Block nichts neu alloziert wird.
    narrow: Vec<Complex32>,
}

impl ChannelReceiver {
    fn new(channel: Channel, offset: f64, sample_rate: f64) -> Self {
        Self {
            channel,
            down: Downconverter::new(offset, sample_rate, CHANNEL_CUTOFF_HZ),
            prev: Complex32::ZERO,
            disc_scale: (NARROW_RATE / (TAU * DEVIATION_HZ)) as f32,
            clock: 0.0,
            last_sign: false,
            last_level: false,
            deframer: Deframer::default(),
            narrow: Vec::new(),
        }
    }

    fn process(&mut self, samples: &[Complex32], out: &mut Vec<ReceivedFrame>) {
        self.narrow.clear();
        self.down.process(samples, &mut self.narrow);
        for i in 0..self.narrow.len() {
            let x = self.narrow[i];
            // Normierte Frequenz: ±1 entspricht dem vollen Hub von ±2,4 kHz.
            let f = (x * self.prev.conj()).arg() * self.disc_scale;
            self.prev = x;
            if let Some(payload) = self.symbol(f) {
                out.push(ReceivedFrame { channel: self.channel, payload });
            }
        }
    }

    /// Ein Sample nach dem Diskriminator, `clock` zählt in Bitdauern.
    fn symbol(&mut self, f: f32) -> Option<Vec<bool>> {
        let sign = f > 0.0;
        self.clock += 1.0 / SPS;
        if sign != self.last_sign {
            // Nulldurchgänge gehören genau zwischen zwei Abtastzeitpunkte.
            self.clock -= (self.clock - 0.5) * CLOCK_GAIN;
            self.last_sign = sign;
        }
        if self.clock < 1.0 {
            return None;
        }
        self.clock -= 1.0;
        // NRZI: gleicher Pegel heißt 1, Wechsel heißt 0.
        let bit = sign == self.last_level;
        self.last_level = sign;
        self.deframer.push(bit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::message::examples::{position, static_data};
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use rand_distr::{Distribution, Normal};

    const FS: f64 = 2.4e6;
    const FC: f64 = 162.0e6;

    /// Moduliert einen Rahmen auf den Kanal und legt ihn in ein Stück Stille.
    fn transmit(signal: &mut [Complex32], start: usize, payload: &[bool], ch: Channel, freq_error: f64, amp: f32) {
        let freq = crate::ais::modulate(payload, (FS / BAUD) as usize);
        let mut phase = 0.3f64;
        for (s, f) in signal[start..].iter_mut().zip(freq) {
            phase += TAU * (ch.freq_hz() - FC + freq_error + f as f64) / FS;
            *s += Complex32::from_polar(amp, phase as f32);
        }
    }

    fn noise(len: usize, sigma: f32, seed: u64) -> Vec<Complex32> {
        let mut rng = StdRng::seed_from_u64(seed);
        let n = Normal::new(0.0, sigma).unwrap();
        (0..len).map(|_| Complex32::new(n.sample(&mut rng), n.sample(&mut rng))).collect()
    }

    fn receive(signal: &[Complex32]) -> Vec<ReceivedFrame> {
        let mut rx = AisReceiver::new(FS, FC);
        // In Blöcken wie in der Erfassungsschleife, damit Zustände über Grenzen tragen.
        signal.chunks(96_000).flat_map(|c| rx.process(c)).collect()
    }

    #[test]
    fn dekodiert_beide_kanaele_gleichzeitig() {
        let mut s = noise(240_000, 0.01, 1);
        let (a, b) = (position().encode(), static_data().encode());
        transmit(&mut s, 20_000, &a, Channel::A, 0.0, 0.1);
        transmit(&mut s, 50_123, &b, Channel::B, 0.0, 0.05);
        let mut got = receive(&s);
        got.sort_by_key(|f| f.channel);
        assert_eq!(
            got,
            vec![ReceivedFrame { channel: Channel::A, payload: a }, ReceivedFrame { channel: Channel::B, payload: b },]
        );
    }

    #[test]
    fn verkraftet_frequenzfehler_eines_billigen_empfaengers() {
        // 3 ppm bei 162 MHz sind knapp 500 Hz.
        let mut s = noise(120_000, 0.01, 2);
        let a = position().encode();
        transmit(&mut s, 10_000, &a, Channel::B, 486.0, 0.1);
        assert_eq!(receive(&s).len(), 1);
    }

    #[test]
    fn im_reinen_rauschen_entsteht_kein_rahmen() {
        assert!(receive(&noise(2_400_000, 0.05, 3)).is_empty());
    }

    #[test]
    fn schwaches_signal_wird_noch_dekodiert() {
        // Rauschen über 2,4 MHz, im 20-kHz-Kanal bleibt davon etwa 1/120 übrig:
        // σ = 0,1 bei Amplitude 0,1 heißt im Kanal etwa 18 dB Rauschabstand.
        let mut s = noise(120_000, 0.1, 4);
        transmit(&mut s, 30_000, &position().encode(), Channel::A, 0.0, 0.1);
        assert_eq!(receive(&s).len(), 1);
    }
}
