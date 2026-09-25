//! Persistenz in SQLite: jede empfangene Nachricht als NMEA, der Positions-
//! verlauf pro Schiff und der zusammengeführte letzte Stand jedes Schiffs.

use crate::ais::message::AisMessage;
use api::{MessageLog, TrackPoint, Vessel};
use rusqlite::{Connection, OptionalExtension, Row, params};

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY,
    ts_ms INTEGER NOT NULL,
    channel TEXT NOT NULL,
    mmsi INTEGER NOT NULL,
    msg_type INTEGER NOT NULL,
    nmea TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS positions (
    mmsi INTEGER NOT NULL,
    ts_ms INTEGER NOT NULL,
    lat REAL NOT NULL,
    lon REAL NOT NULL,
    sog REAL,
    cog REAL
);
CREATE INDEX IF NOT EXISTS positions_by_ship ON positions (mmsi, ts_ms);
CREATE TABLE IF NOT EXISTS vessels (
    mmsi INTEGER PRIMARY KEY,
    name TEXT,
    callsign TEXT,
    ship_type INTEGER,
    destination TEXT,
    length_m INTEGER,
    beam_m INTEGER,
    lat REAL,
    lon REAL,
    sog REAL,
    cog REAL,
    heading INTEGER,
    nav_status INTEGER,
    last_seen_ms INTEGER NOT NULL,
    messages INTEGER NOT NULL DEFAULT 0
);
";

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Speichert eine Nachricht und liefert den neuen Stand des Schiffs.
    /// Nicht ausgewertete Typen landen nur im Nachrichtenprotokoll.
    pub fn record(&mut self, log: &MessageLog, msg: Option<&AisMessage>) -> rusqlite::Result<Vessel> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO messages (ts_ms, channel, mmsi, msg_type, nmea) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![log.ts_ms, log.channel.to_string(), log.mmsi, log.msg_type, log.nmea.join("\n")],
        )?;
        tx.execute(
            "INSERT INTO vessels (mmsi, last_seen_ms, messages) VALUES (?1, ?2, 1)
             ON CONFLICT (mmsi) DO UPDATE SET last_seen_ms = excluded.last_seen_ms, messages = messages + 1",
            params![log.mmsi, log.ts_ms],
        )?;
        match msg {
            Some(AisMessage::Position(p)) => {
                tx.execute(
                    "UPDATE vessels SET lat = ?2, lon = ?3, sog = ?4, cog = ?5, heading = ?6, nav_status = ?7
                     WHERE mmsi = ?1",
                    params![p.mmsi, p.lat, p.lon, p.sog, p.cog, p.heading, p.nav_status],
                )?;
                if let (Some(lat), Some(lon)) = (p.lat, p.lon) {
                    tx.execute(
                        "INSERT INTO positions (mmsi, ts_ms, lat, lon, sog, cog) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![p.mmsi, log.ts_ms, lat, lon, p.sog, p.cog],
                    )?;
                }
            }
            Some(AisMessage::Static(s)) => {
                tx.execute(
                    "UPDATE vessels SET name = ?2, callsign = ?3, ship_type = ?4, destination = ?5,
                     length_m = ?6, beam_m = ?7 WHERE mmsi = ?1",
                    params![s.mmsi, s.name, s.callsign, s.ship_type, s.destination, s.length(), s.beam()],
                )?;
            }
            None => {}
        }
        let vessel = tx
            .query_row(&format!("SELECT {VESSEL_COLUMNS} FROM vessels WHERE mmsi = ?1"), [log.mmsi], vessel_from_row)
            .optional()?
            .expect("Zeile wurde gerade geschrieben");
        tx.commit()?;
        Ok(vessel)
    }

    pub fn vessels(&self) -> rusqlite::Result<Vec<Vessel>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {VESSEL_COLUMNS} FROM vessels ORDER BY mmsi"))?;
        stmt.query_map([], vessel_from_row)?.collect()
    }

    /// Die letzten `limit` Positionen, älteste zuerst.
    pub fn track(&self, mmsi: u32, limit: u32) -> rusqlite::Result<Vec<TrackPoint>> {
        let mut stmt = self.conn.prepare(
            "SELECT ts_ms, lat, lon FROM (
                 SELECT ts_ms, lat, lon FROM positions WHERE mmsi = ?1 ORDER BY ts_ms DESC LIMIT ?2
             ) ORDER BY ts_ms",
        )?;
        stmt.query_map(params![mmsi, limit], |r| Ok(TrackPoint { ts_ms: r.get(0)?, lat: r.get(1)?, lon: r.get(2)? }))?
            .collect()
    }

    /// Die letzten `limit` Nachrichten, neueste zuerst.
    pub fn messages(&self, limit: u32) -> rusqlite::Result<Vec<MessageLog>> {
        let mut stmt =
            self.conn.prepare("SELECT ts_ms, channel, mmsi, msg_type, nmea FROM messages ORDER BY id DESC LIMIT ?1")?;
        stmt.query_map([limit], |r| {
            Ok(MessageLog {
                ts_ms: r.get(0)?,
                channel: r.get::<_, String>(1)?.chars().next().unwrap_or('?'),
                mmsi: r.get(2)?,
                msg_type: r.get(3)?,
                nmea: r.get::<_, String>(4)?.lines().map(str::to_string).collect(),
            })
        })?
        .collect()
    }
}

const VESSEL_COLUMNS: &str = "mmsi, name, callsign, ship_type, destination, length_m, beam_m, \
     lat, lon, sog, cog, heading, nav_status, last_seen_ms, messages";

fn vessel_from_row(r: &Row) -> rusqlite::Result<Vessel> {
    Ok(Vessel {
        mmsi: r.get(0)?,
        name: r.get(1)?,
        callsign: r.get(2)?,
        ship_type: r.get(3)?,
        destination: r.get(4)?,
        length_m: r.get(5)?,
        beam_m: r.get(6)?,
        lat: r.get(7)?,
        lon: r.get(8)?,
        sog: r.get(9)?,
        cog: r.get(10)?,
        heading: r.get(11)?,
        nav_status: r.get(12)?,
        last_seen_ms: r.get(13)?,
        messages: r.get(14)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::message::tests::{position, static_data};

    fn log(ts_ms: i64, msg_type: u8) -> MessageLog {
        MessageLog { ts_ms, channel: 'A', mmsi: 211_234_560, msg_type, nmea: vec!["!AIVDM,…".into()] }
    }

    #[test]
    fn position_und_stammdaten_werden_zusammengefuehrt() {
        let mut db = Store::open(":memory:").unwrap();
        db.record(&log(1000, 1), Some(&position())).unwrap();
        let v = db.record(&log(2000, 5), Some(&static_data())).unwrap();
        assert_eq!(v.name.as_deref(), Some("FOERDE EXPRESS"));
        assert_eq!(v.length_m, Some(70));
        assert!((v.lat.unwrap() - 54.3721).abs() < 1e-6);
        assert_eq!((v.messages, v.last_seen_ms), (2, 2000));
        assert_eq!(db.vessels().unwrap(), vec![v]);
    }

    #[test]
    fn verlauf_liefert_die_juengsten_punkte_in_zeitfolge() {
        let mut db = Store::open(":memory:").unwrap();
        for t in 1..=5 {
            db.record(&log(t * 1000, 1), Some(&position())).unwrap();
        }
        let track = db.track(211_234_560, 3).unwrap();
        assert_eq!(track.iter().map(|p| p.ts_ms).collect::<Vec<_>>(), vec![3000, 4000, 5000]);
        let msgs = db.messages(2).unwrap();
        assert_eq!(msgs[0].ts_ms, 5000);
        assert_eq!(msgs[0].nmea, log(0, 1).nmea);
    }
}
