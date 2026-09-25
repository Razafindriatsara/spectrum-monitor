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
