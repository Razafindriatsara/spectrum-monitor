//! AIS: Automatic Identification System. Schiffe senden damit Position,
//! Kurs und Stammdaten auf zwei UKW-Kanälen, GMSK-moduliert mit 9600 Bit/s.
//!
//! Die Module bilden die Kette in beide Richtungen ab:
//! [`message`] ↔ Bits, [`nmea`] für die Ausgabe an andere Programme.

mod bits;
pub mod message;
pub mod nmea;
