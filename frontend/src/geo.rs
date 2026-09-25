//! Web-Mercator wie bei OpenStreetMap: Die Welt ist bei Zoomstufe z ein
//! Quadrat aus 2^z × 2^z Kacheln zu 256 Pixeln.

use std::f64::consts::PI;

pub const TILE: f64 = 256.0;
pub const MIN_ZOOM: u8 = 3;
pub const MAX_ZOOM: u8 = 18;

fn world_size(zoom: u8) -> f64 {
    TILE * f64::from(1u32 << zoom)
}

/// Breite und Länge in Grad zu Weltpixeln.
pub fn project(lat: f64, lon: f64, zoom: u8) -> (f64, f64) {
    let s = world_size(zoom);
    let phi = lat.clamp(-85.05, 85.05).to_radians();
    let x = (lon + 180.0) / 360.0 * s;
    let y = (1.0 - (phi.tan() + 1.0 / phi.cos()).ln() / PI) / 2.0 * s;
    (x, y)
}

pub fn unproject(x: f64, y: f64, zoom: u8) -> (f64, f64) {
    let s = world_size(zoom);
    let lon = x / s * 360.0 - 180.0;
    let lat = (PI * (1.0 - 2.0 * y / s)).sinh().atan().to_degrees();
    (lat, lon)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tile {
    pub z: u8,
    /// Spalte ohne Umbruch an der Datumsgrenze, bestimmt die Lage auf dem Bildschirm.
    pub x: i64,
    pub y: i64,
}

impl Tile {
    pub fn url(&self) -> String {
        let n = 1i64 << self.z;
        format!("https://tile.openstreetmap.org/{}/{}/{}.png", self.z, self.x.rem_euclid(n), self.y)
    }
}

/// Alle Kacheln, die ein Ausschnitt ab Weltpixel `origin` mit `size` berührt.
pub fn visible_tiles(origin: (f64, f64), size: (f64, f64), zoom: u8) -> Vec<Tile> {
    let n = 1i64 << zoom;
    let x0 = (origin.0 / TILE).floor() as i64;
    let x1 = ((origin.0 + size.0) / TILE).floor() as i64;
    let y0 = ((origin.1 / TILE).floor() as i64).max(0);
    let y1 = (((origin.1 + size.1) / TILE).floor() as i64).min(n - 1);
    (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| Tile { z: zoom, x, y })).collect()
}
