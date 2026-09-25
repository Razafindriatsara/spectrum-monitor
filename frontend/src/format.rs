//! Zahlen und Zeiten in deutscher Schreibweise.

use wasm_bindgen::JsValue;

/// Dezimalkomma und echtes Minuszeichen.
pub fn num(v: f64, decimals: usize) -> String {
    format!("{v:.decimals$}").replace('.', ",").replace('-', "−")
}

pub fn mhz(hz: f64) -> String {
    format!("{} MHz", num(hz / 1e6, 3))
}

pub fn dbfs(v: f32) -> String {
    format!("{} dBFS", num(v.into(), 1))
}

/// Grad und Dezimalminuten wie auf der Seekarte: 54° 22,334′ N.
pub fn lat_lon(lat: f64, lon: f64) -> String {
    let part = |v: f64, pos: char, neg: char, width: usize| {
        let a = v.abs();
        let deg = a.trunc();
        format!("{:0width$}° {}′ {}", deg as u32, num((a - deg) * 60.0, 3), if v >= 0.0 { pos } else { neg })
    };
    format!("{}  {}", part(lat, 'N', 'S', 2), part(lon, 'E', 'W', 3))
}

/// Uhrzeit in der Zeitzone des Browsers.
pub fn clock(ms: i64) -> String {
    let d = js_sys::Date::new(&JsValue::from_f64(ms as f64));
    format!("{:02}:{:02}:{:02}", d.get_hours(), d.get_minutes(), d.get_seconds())
}

pub fn age(now_ms: f64, then_ms: i64) -> String {
    let s = ((now_ms - then_ms as f64) / 1000.0).max(0.0) as i64;
    match s {
        0..60 => format!("vor {s} s"),
        60..3600 => format!("vor {} min", s / 60),
        _ => format!("vor {} h", s / 3600),
    }
}
