//! Simulierte Schiffe in der Kieler Förde. Namen und MMSI sind erfunden. Die
//! Wegpunkte liegen im Wasser, geprüft gegen die Wasserfläche der OSM-Karte.
//! Die Meldeintervalle sind kürzer als in der Norm, damit sich die Karte
//! schnell füllt.

use super::Channel;
use super::message::{AisMessage, PositionReport, StaticData};
use rand::RngExt;
use rand::rngs::StdRng;

/// Standort der simulierten Empfangsantenne, bestimmt die Signalstärke.
const RECEIVER: (f64, f64) = (54.3290, 10.1490);

const FAIRWAY: &[(f64, f64)] = &[
    (54.3195, 10.1445),
    (54.3280, 10.1560),
    (54.3452, 10.1650),
    (54.3600, 10.1720),
    (54.3750, 10.1800),
    (54.3870, 10.1960),
    (54.3970, 10.2060),
    (54.4080, 10.2070),
    (54.4300, 10.2350),
    (54.4600, 10.2600),
    (54.5000, 10.2750),
];
const CANAL: &[(f64, f64)] = &[
    (54.3650, 10.1570),
    (54.3680, 10.1680),
    (54.3750, 10.1800),
    (54.3870, 10.1960),
    (54.3970, 10.2060),
    (54.4080, 10.2070),
    (54.4300, 10.2350),
    (54.4700, 10.2900),
];
const FERRY: &[(f64, f64)] = &[
    (54.3209, 10.1434),
    (54.3302, 10.1548),
    (54.3500, 10.1690),
    (54.3740, 10.1830),
    (54.3860, 10.1990),
    (54.4008, 10.2144),
];
const FISHING: &[(f64, f64)] = &[(54.4550, 10.2250), (54.4700, 10.2400), (54.4600, 10.2750), (54.4450, 10.2500)];
const SAILING: &[(f64, f64)] = &[(54.4350, 10.1900), (54.4450, 10.2150), (54.4300, 10.2250), (54.4200, 10.2050)];

struct Ship {
    data: StaticData,
    speed_kn: f32,
    status: u8,
    route: Route,
    /// Zurückgelegte Strecke auf der Route in Metern.
    along: f64,
    forward: bool,
    next_position: f64,
    next_static: f64,
    channel: Channel,
}

struct Route {
    points: Vec<(f64, f64)>,
    /// Kumulierte Länge bis zu jedem Punkt in Metern.
    cum: Vec<f64>,
    /// Rundkurs statt Hin- und Rückfahrt.
    closed: bool,
}

pub struct Fleet {
    ships: Vec<Ship>,
    time: f64,
}

impl Fleet {
    pub fn kiel(rng: &mut StdRng) -> Self {
        let mut ships = Vec::new();
        let mut add = |mmsi, name: &str, callsign: &str, ship_type, len: u16, beam: u16, speed_kn, status, route| {
            ships.push(Ship {
                data: StaticData {
                    mmsi,
                    imo: 0,
                    callsign: callsign.into(),
                    name: name.into(),
                    ship_type,
                    dims: [len * 2 / 3, len - len * 2 / 3, beam / 2, beam - beam / 2],
                    draught: (f32::from(len) / 25.0).clamp(1.5, 8.0),
                    destination: "KIEL".into(),
                },
                speed_kn,
                status,
                along: 0.0,
                forward: true,
                next_position: rng.random_range(0.2..3.0),
                next_static: rng.random_range(0.5..5.0),
                channel: if rng.random_bool(0.5) { Channel::A } else { Channel::B },
                route,
            });
        };
        add(211_990_001, "NORDLICHT", "DSIM1", 60, 190, 30, 12.0, 0, Route::open(FAIRWAY));
        add(211_990_002, "SCHWENTINE", "DSIM2", 70, 110, 17, 9.5, 0, Route::open(CANAL));
        add(211_990_003, "LABOE", "DSIM3", 60, 35, 9, 8.0, 0, Route::open(FERRY));
        add(211_990_004, "BALTIC AMBER", "DSIM4", 80, 145, 22, 0.0, 1, Route::open(&[(54.4420, 10.2620)]));
        add(211_990_005, "SPROTTE", "DSIM5", 30, 22, 7, 5.0, 7, Route::closed(FISHING));
        add(211_990_006, "MOEWE", "DSIM6", 36, 12, 4, 5.5, 8, Route::closed(SAILING));

        // Nicht alle am Anfang ihrer Route starten lassen.
        for s in &mut ships {
            s.along = rng.random_range(0.0..=s.route.length());
            s.forward = rng.random_bool(0.5);
        }
        Self { ships, time: 0.0 }
    }

    /// Bewegt die Schiffe um `dt` Sekunden weiter und liefert alle Nachrichten,
    /// die in dieser Zeit fällig werden, mit Kanal und Empfangsamplitude.
    pub fn advance(&mut self, dt: f64, rng: &mut StdRng) -> Vec<(AisMessage, Channel, f32)> {
        self.time += dt;
        let mut out = Vec::new();
        for s in &mut self.ships {
            s.step(dt);
            let (lat, lon, course) = s.route.at(s.along, s.forward);
            let amplitude = amplitude_at(lat, lon);

            if self.time >= s.next_static {
                s.next_static = self.time + rng.random_range(25.0..35.0);
                out.push((AisMessage::Static(s.data.clone()), s.next_channel(), amplitude));
            }
            if self.time >= s.next_position {
                let underway = s.speed_kn > 0.0;
                s.next_position =
                    self.time + if underway { rng.random_range(3.0..5.0) } else { rng.random_range(10.0..14.0) };
                let cog = if underway { course + rng.random_range(-1.5..1.5) } else { 0.0 };
                let msg = AisMessage::Position(PositionReport {
                    msg_type: 1,
                    mmsi: s.data.mmsi,
                    nav_status: s.status,
                    sog: Some(s.speed_kn + if underway { rng.random_range(-0.2..0.2) } else { 0.0 }),
                    accurate: true,
                    lon: Some(lon),
                    lat: Some(lat),
                    cog: Some(cog.rem_euclid(360.0) as f32),
                    heading: underway.then(|| course.round().rem_euclid(360.0) as u16),
                    second: (self.time % 60.0) as u8,
                });
                out.push((msg, s.next_channel(), amplitude));
            }
        }
        out
    }
}

impl Ship {
    fn step(&mut self, dt: f64) {
        let len = self.route.length();
        if len == 0.0 {
            return;
        }
        let d = f64::from(self.speed_kn) * 1852.0 / 3600.0 * dt;
        if self.route.closed {
            self.along = (self.along + d).rem_euclid(len);
            return;
        }
        // Am Ende der Route wenden.
        self.along += if self.forward { d } else { -d };
        if self.along >= len {
            self.along = 2.0 * len - self.along;
            self.forward = false;
        } else if self.along <= 0.0 {
            self.along = -self.along;
            self.forward = true;
        }
    }

    /// Klasse-A-Geräte wechseln die Kanäle bei jeder Aussendung ab.
    fn next_channel(&mut self) -> Channel {
        let ch = self.channel;
        self.channel = if ch == Channel::A { Channel::B } else { Channel::A };
        ch
    }
}

impl Route {
    fn open(points: &[(f64, f64)]) -> Self {
        Self::new(points.to_vec(), false)
    }

    fn closed(points: &[(f64, f64)]) -> Self {
        let mut p = points.to_vec();
        p.push(points[0]);
        Self::new(p, true)
    }

    fn new(points: Vec<(f64, f64)>, closed: bool) -> Self {
        let mut cum = vec![0.0];
        for w in points.windows(2) {
            cum.push(cum.last().unwrap() + distance(w[0], w[1]));
        }
        Self { points, cum, closed }
    }

    fn length(&self) -> f64 {
        *self.cum.last().unwrap()
    }

    /// Position und Kurs in Grad an einer Stelle der Route.
    fn at(&self, along: f64, forward: bool) -> (f64, f64, f64) {
        if self.points.len() == 1 {
            return (self.points[0].0, self.points[0].1, 0.0);
        }
        let i = self.cum.partition_point(|&c| c <= along).clamp(1, self.points.len() - 1);
        let (a, b) = (self.points[i - 1], self.points[i]);
        let t = ((along - self.cum[i - 1]) / (self.cum[i] - self.cum[i - 1])).clamp(0.0, 1.0);
        let pos = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        let course = if forward { bearing(a, b) } else { bearing(b, a) };
        (pos.0, pos.1, course)
    }
}

/// Lokale Näherung, auf wenigen Kilometern genau genug.
fn to_meters(from: (f64, f64), to: (f64, f64)) -> (f64, f64) {
    let north = (to.0 - from.0) * 111_320.0;
    let east = (to.1 - from.1) * 111_320.0 * from.0.to_radians().cos();
    (north, east)
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (n, e) = to_meters(a, b);
    n.hypot(e)
}

fn bearing(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (n, e) = to_meters(a, b);
    e.atan2(n).to_degrees().rem_euclid(360.0)
}

/// Pegel sinkt mit der Entfernung zur Antenne, nahe Schiffe sind stark.
fn amplitude_at(lat: f64, lon: f64) -> f32 {
    let d = distance(RECEIVER, (lat, lon)).max(1500.0);
    (0.12 * 1500.0 / d) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn jedes_schiff_meldet_position_und_name() {
        let mut rng = StdRng::seed_from_u64(1);
        let mut fleet = Fleet::kiel(&mut rng);
        let msgs: Vec<_> = (0..400).flat_map(|_| fleet.advance(0.1, &mut rng)).collect();
        for s in &fleet.ships {
            let mine = |t| msgs.iter().any(|(m, _, _)| m.mmsi() == s.data.mmsi && m.msg_type() == t);
            assert!(mine(1) && mine(5), "{} fehlt", s.data.name);
        }
    }

    #[test]
    fn schiff_wendet_am_routenende() {
        let s = Route::open(&[(54.0, 10.0), (54.01, 10.0)]);
        let mut ship = Ship {
            data: match crate::ais::message::tests::static_data() {
                AisMessage::Static(d) => d,
                _ => unreachable!(),
            },
            speed_kn: 10.0,
            status: 0,
            along: 1000.0,
            forward: true,
            next_position: 0.0,
            next_static: 0.0,
            channel: Channel::A,
            route: s,
        };
        ship.step(60.0); // gut 300 m weiter, die Route ist gut 1100 m lang
        assert!(!ship.forward);
        let (_, _, course) = ship.route.at(ship.along, ship.forward);
        assert!((course - 180.0).abs() < 1.0);
    }
}
