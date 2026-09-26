//! AIS: Automatic Identification System. Schiffe senden damit Position,
//! Kurs und Stammdaten auf zwei UKW-Kanälen, GMSK-moduliert mit 9600 Bit/s.
//!
//! Die Module bilden die Kette in beide Richtungen ab:
//! [`message`] ↔ Bits, [`frame`] ↔ HDLC und NRZI, [`gmsk`] für den Simulator,
//! [`demod`] für den Empfang, [`nmea`] für die Ausgabe an andere Programme.

mod bits;
pub mod demod;
pub mod fleet;
pub mod frame;
pub mod gmsk;
pub mod message;
pub mod nmea;

pub use demod::{AisReceiver, ReceivedFrame};
pub use message::AisMessage;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Channel {
    /// 161,975 MHz (UKW-Kanal 87B)
    A,
    /// 162,025 MHz (UKW-Kanal 88B)
    B,
}

impl Channel {
    pub fn freq_hz(self) -> f64 {
        match self {
            Channel::A => 161.975e6,
            Channel::B => 162.025e6,
        }
    }

    pub fn letter(self) -> char {
        match self {
            Channel::A => 'A',
            Channel::B => 'B',
        }
    }
}

/// Frequenzverlauf eines kompletten Bursts für den Simulator: Rahmen bauen,
/// NRZI-kodieren, GMSK-modulieren. Hinten hängen ein paar Bits ohne Wechsel,
/// damit das Gaußfilter das schließende Flag nicht abschneidet.
pub fn modulate(payload: &[bool], samples_per_bit: usize) -> Vec<f32> {
    let mut levels = frame::nrzi(&frame::build(payload));
    let last = *levels.last().expect("Rahmen ist nie leer");
    levels.extend([last; 4]);
    gmsk::frequency(&levels, samples_per_bit)
}
