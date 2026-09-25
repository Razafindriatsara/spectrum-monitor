//! AIS-Nachrichten nach ITU-R M.1371: Positionsmeldung Klasse A (Typ 1–3)
//! und Stamm- und Reisedaten (Typ 5). Andere Typen werden erkannt, aber nicht
//! ausgewertet.

use super::bits::{BitReader, BitWriter};

#[derive(Clone, Debug, PartialEq)]
pub enum AisMessage {
    Position(PositionReport),
    Static(StaticData),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PositionReport {
    /// 1 und 2 bei SOTDMA/ITDMA-Planung, 3 als Antwort auf eine Abfrage.
    pub msg_type: u8,
    pub mmsi: u32,
    pub nav_status: u8,
    /// Knoten, Auflösung 0,1.
    pub sog: Option<f32>,
    pub accurate: bool,
    /// Grad, Auflösung 1/10 000 Bogenminute.
    pub lon: Option<f64>,
    pub lat: Option<f64>,
    /// Grad, Auflösung 0,1.
    pub cog: Option<f32>,
    pub heading: Option<u16>,
    /// Sekunde des UTC-Zeitstempels (60 = nicht verfügbar).
    pub second: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StaticData {
    pub mmsi: u32,
    pub imo: u32,
    pub callsign: String,
    pub name: String,
    pub ship_type: u8,
    /// Abstand der Antenne zu Bug, Heck, Backbord, Steuerbord in Metern.
    pub dims: [u16; 4],
    /// Meter, Auflösung 0,1.
    pub draught: f32,
    pub destination: String,
}

impl StaticData {
    pub fn length(&self) -> u16 {
        self.dims[0] + self.dims[1]
    }

    pub fn beam(&self) -> u16 {
        self.dims[2] + self.dims[3]
    }
}

#[derive(Debug, PartialEq)]
pub enum DecodeError {
    TooShort,
    Unsupported(u8),
}

// Werte für "nicht verfügbar".
const LON_NA: i64 = 181 * 600_000;
const LAT_NA: i64 = 91 * 600_000;
const SOG_NA: u64 = 1023;
const COG_NA: u64 = 3600;
const HDG_NA: u64 = 511;

impl AisMessage {
    #[cfg(test)]
    pub fn mmsi(&self) -> u32 {
        match self {
            AisMessage::Position(p) => p.mmsi,
            AisMessage::Static(s) => s.mmsi,
        }
    }

    #[cfg(test)]
    pub fn msg_type(&self) -> u8 {
        match self {
            AisMessage::Position(p) => p.msg_type,
            AisMessage::Static(_) => 5,
        }
    }

    pub fn encode(&self) -> Vec<bool> {
        let mut w = BitWriter::default();
        match self {
            AisMessage::Position(p) => {
                w.uint(p.msg_type.into(), 6);
                w.uint(0, 2); // Wiederholungszähler
                w.uint(p.mmsi.into(), 30);
                w.uint(p.nav_status.into(), 4);
                w.int(-128, 8); // Drehrate nicht verfügbar
                w.uint(p.sog.map_or(SOG_NA, |v| (v * 10.0).round() as u64), 10);
                w.uint(p.accurate.into(), 1);
                w.int(p.lon.map_or(LON_NA, |v| (v * 600_000.0).round() as i64), 28);
                w.int(p.lat.map_or(LAT_NA, |v| (v * 600_000.0).round() as i64), 27);
                w.uint(p.cog.map_or(COG_NA, |v| (v * 10.0).round() as u64), 12);
                w.uint(p.heading.map_or(HDG_NA, u64::from), 9);
                w.uint(p.second.into(), 6);
                w.uint(0, 2); // Manöverkennung
                w.uint(0, 3); // frei
                w.uint(0, 1); // RAIM
                w.uint(0, 19); // Funkstatus
            }
            AisMessage::Static(s) => {
                w.uint(5, 6);
                w.uint(0, 2);
                w.uint(s.mmsi.into(), 30);
                w.uint(0, 2); // AIS-Version
                w.uint(s.imo.into(), 30);
                w.text(&s.callsign, 7);
                w.text(&s.name, 20);
                w.uint(s.ship_type.into(), 8);
                w.uint(s.dims[0].into(), 9);
                w.uint(s.dims[1].into(), 9);
                w.uint(s.dims[2].into(), 6);
                w.uint(s.dims[3].into(), 6);
                w.uint(1, 4); // Positionsquelle GPS
                w.uint(0, 4); // ETA nicht verfügbar: Monat 0, Tag 0, 24:60 Uhr
                w.uint(0, 5);
                w.uint(24, 5);
                w.uint(60, 6);
                w.uint((s.draught * 10.0).round() as u64, 8);
                w.text(&s.destination, 20);
                w.uint(0, 1); // Datenendgerät bereit
                w.uint(0, 1);
            }
        }
        w.into_bits()
    }

    pub fn decode(bits: &[bool]) -> Result<Self, DecodeError> {
        let mut r = BitReader::new(bits);
        let msg_type = r.uint(6).ok_or(DecodeError::TooShort)? as u8;
        match msg_type {
            1..=3 => decode_position(msg_type, &mut r).ok_or(DecodeError::TooShort),
            5 => decode_static(&mut r).ok_or(DecodeError::TooShort),
            t => Err(DecodeError::Unsupported(t)),
        }
    }
}

/// MMSI aus beliebigen Nachrichtentypen, sie steht immer in Bit 8 bis 37.
pub fn peek_mmsi(bits: &[bool]) -> Option<u32> {
    let mut r = BitReader::new(bits);
    r.uint(8)?;
    r.uint(30).map(|v| v as u32)
}

fn decode_position(msg_type: u8, r: &mut BitReader) -> Option<AisMessage> {
    r.uint(2)?;
    let mmsi = r.uint(30)? as u32;
    let nav_status = r.uint(4)? as u8;
    r.int(8)?;
    let sog = r.uint(10)?;
    let accurate = r.uint(1)? == 1;
    let lon = r.int(28)?;
    let lat = r.int(27)?;
    let cog = r.uint(12)?;
    let heading = r.uint(9)?;
    let second = r.uint(6)? as u8;
    Some(AisMessage::Position(PositionReport {
        msg_type,
        mmsi,
        nav_status,
        sog: (sog != SOG_NA).then(|| sog as f32 / 10.0),
        accurate,
        lon: (lon != LON_NA).then(|| lon as f64 / 600_000.0),
        lat: (lat != LAT_NA).then(|| lat as f64 / 600_000.0),
        cog: (cog < COG_NA).then(|| cog as f32 / 10.0),
        heading: (heading != HDG_NA).then_some(heading as u16),
        second,
    }))
}

fn decode_static(r: &mut BitReader) -> Option<AisMessage> {
    r.uint(2)?;
    let mmsi = r.uint(30)? as u32;
    r.uint(2)?;
    let imo = r.uint(30)? as u32;
    let callsign = r.text(7)?;
    let name = r.text(20)?;
    let ship_type = r.uint(8)? as u8;
    let dims = [r.uint(9)? as u16, r.uint(9)? as u16, r.uint(6)? as u16, r.uint(6)? as u16];
    r.uint(4 + 4 + 5 + 5 + 6)?; // Positionsquelle und ETA
    let draught = r.uint(8)? as f32 / 10.0;
    let destination = r.text(20)?;
    Some(AisMessage::Static(StaticData { mmsi, imo, callsign, name, ship_type, dims, draught, destination }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn position() -> AisMessage {
        AisMessage::Position(PositionReport {
            msg_type: 1,
            mmsi: 211_234_560,
            nav_status: 0,
            sog: Some(12.3),
            accurate: true,
            lon: Some(10.1834),
            lat: Some(54.3721),
            cog: Some(17.5),
            heading: Some(18),
            second: 42,
        })
    }

    pub fn static_data() -> AisMessage {
        AisMessage::Static(StaticData {
            mmsi: 211_234_560,
            imo: 9_123_456,
            callsign: "DABC".into(),
            name: "FOERDE EXPRESS".into(),
            ship_type: 60,
            dims: [30, 40, 8, 8],
            draught: 4.2,
            destination: "KIEL".into(),
        })
    }

    fn close(a: Option<f64>, b: Option<f64>) -> bool {
        (a.unwrap() - b.unwrap()).abs() < 1e-6
    }

    #[test]
    fn positionsmeldung_ueberlebt_hin_und_rueckweg() {
        let msg = position();
        let bits = msg.encode();
        assert_eq!(bits.len(), 168);
        let AisMessage::Position(back) = AisMessage::decode(&bits).unwrap() else { panic!() };
        let AisMessage::Position(orig) = msg else { panic!() };
        assert!(close(back.lat, orig.lat) && close(back.lon, orig.lon));
        assert_eq!((back.mmsi, back.sog, back.cog, back.heading), (orig.mmsi, orig.sog, orig.cog, orig.heading));
    }

    #[test]
    fn stammdaten_ueberleben_hin_und_rueckweg() {
        let msg = static_data();
        let bits = msg.encode();
        assert_eq!(bits.len(), 424);
        assert_eq!(AisMessage::decode(&bits).unwrap(), msg);
        assert_eq!(peek_mmsi(&bits), Some(211_234_560));
    }

    #[test]
    fn unbekannter_typ_wird_gemeldet() {
        let mut w = BitWriter::default();
        w.uint(21, 6);
        w.uint(0, 162);
        assert_eq!(AisMessage::decode(&w.into_bits()), Err(DecodeError::Unsupported(21)));
    }
}
