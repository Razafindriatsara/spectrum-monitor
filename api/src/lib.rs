//! Datentypen, die Server und Frontend teilen. Beide Seiten serialisieren
//! mit denselben Strukturen, damit das Protokoll nicht auseinanderläuft.
//!
//! Endpunkte:
//! - `GET /ws`: WebSocket mit [`SpectrumMeta`] als Text, danach pro Frame
//!   `f32`-Werte in dBFS als Binärnachricht, Little Endian.
//! - `GET /ws/ais`: WebSocket mit einem [`AisEvent`] (JSON) pro dekodierter Nachricht.
//! - `GET /api/vessels`: alle bekannten Schiffe als `Vec<Vessel>`.
//! - `GET /api/vessels/{mmsi}/track?limit=N`: Positionsverlauf als `Vec<TrackPoint>`.
//! - `GET /api/messages?limit=N`: letzte Nachrichten als `Vec<MessageLog>`, neueste zuerst.

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

impl Vessel {
    /// Name, falls bekannt, sonst die MMSI.
    pub fn label(&self) -> String {
        match &self.name {
            Some(n) if !n.is_empty() => n.clone(),
            _ => self.mmsi.to_string(),
        }
    }

    /// Richtung für die Kartendarstellung: Steuerkurs, ersatzweise Kurs über Grund.
    pub fn bearing(&self) -> Option<f32> {
        self.heading.map(f32::from).or(self.cog)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackPoint {
    pub ts_ms: i64,
    pub lat: f64,
    pub lon: f64,
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

/// Wird pro dekodierter Nachricht über `/ws/ais` verteilt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AisEvent {
    pub message: MessageLog,
    /// Stand des Schiffs nach dieser Nachricht.
    pub vessel: Vessel,
}

/// Navigationsstatus nach ITU-R M.1371.
pub fn nav_status_text(status: u8) -> &'static str {
    match status {
        0 => "In Fahrt unter Maschine",
        1 => "Vor Anker",
        2 => "Manövrierunfähig",
        3 => "Manövrierbehindert",
        4 => "Tiefgangbehindert",
        5 => "Festgemacht",
        6 => "Auf Grund",
        7 => "Beim Fischen",
        8 => "In Fahrt unter Segeln",
        _ => "Nicht angegeben",
    }
}

/// Grobe Schiffsart aus dem AIS-Typcode.
pub fn ship_type_text(code: u8) -> &'static str {
    match code {
        30 => "Fischerei",
        31 | 32 | 52 => "Schlepper",
        35 => "Marine",
        36 => "Segelfahrzeug",
        37 => "Sportboot",
        40..=49 => "Hochgeschwindigkeitsfahrzeug",
        50 => "Lotsenfahrzeug",
        51 => "Such- und Rettungsfahrzeug",
        53 => "Hafenfahrzeug",
        55 => "Behördenfahrzeug",
        60..=69 => "Fahrgastschiff",
        70..=79 => "Frachtschiff",
        80..=89 => "Tanker",
        _ => "Sonstiges",
    }
}
