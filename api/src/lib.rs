//! Datentypen, die Server und Frontend teilen. Beide Seiten serialisieren
//! mit denselben Strukturen, damit das Protokoll nicht auseinanderläuft.
//!
//! Endpunkte:
//! - `GET /ws`: WebSocket mit [`SpectrumMeta`] als Text, danach pro Frame
//!   `f32`-Werte in dBFS als Binärnachricht, Little Endian.

use serde::{Deserialize, Serialize};

/// Beschreibt den Spektrumstrom; wird einmal pro Verbindung gesendet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpectrumMeta {
    pub center_hz: f64,
    pub sample_rate: f64,
    pub fft_size: usize,
    pub rbw_hz: f64,
    pub fps: f64,
    /// `"Peak"` oder `"Avg"`.
    pub detector: String,
}

/// Letzter bekannter Stand eines Schiffs, zusammengeführt aus Positions-
/// und Stammdatenmeldungen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vessel {
    pub mmsi: u32,
    pub name: Option<String>,
    pub callsign: Option<String>,
    pub ship_type: Option<u8>,
    pub destination: Option<String>,
    pub length_m: Option<u16>,
    pub beam_m: Option<u16>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// Fahrt über Grund in Knoten.
    pub sog: Option<f32>,
    /// Kurs über Grund in Grad.
    pub cog: Option<f32>,
    /// Rechtweisender Steuerkurs in Grad.
    pub heading: Option<u16>,
    pub nav_status: Option<u8>,
    pub last_seen_ms: i64,
    pub messages: u32,
}

/// Eine empfangene Nachricht, wie sie im Ereignisprotokoll steht.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MessageLog {
    pub ts_ms: i64,
    /// `'A'` (161,975 MHz) oder `'B'` (162,025 MHz).
    pub channel: char,
    pub mmsi: u32,
    pub msg_type: u8,
    /// NMEA-0183-Sätze (`!AIVDM`), wie sie auch per UDP ausgegeben werden.
    pub nmea: Vec<String>,
}
