//! NMEA 0183: AIS-Nutzdaten als `!AIVDM`-Sätze, wie sie Kartenplotter wie
//! OpenCPN erwarten. Je 6 Bit werden zu einem druckbaren Zeichen.

/// Höchstens so viele Zeichen Nutzdaten pro Satz, damit er unter 82 Zeichen bleibt.
const MAX_CHARS: usize = 60;

pub fn armor(bits: &[bool]) -> (String, u8) {
    let fill = (6 - bits.len() % 6) % 6;
    let s = bits
        .chunks(6)
        .map(|c| {
            let v = c.iter().chain(std::iter::repeat(&false)).take(6).fold(0u8, |a, &b| a << 1 | u8::from(b));
            char::from(if v < 40 { v + 48 } else { v + 56 })
        })
        .collect();
    (s, fill as u8)
}

/// Gegenrichtung zu [`armor`]; bisher nur in Tests gegen echte Sätze genutzt.
#[cfg(test)]
pub fn unarmor(payload: &str, fill: u8) -> Option<Vec<bool>> {
    let mut bits = Vec::with_capacity(payload.len() * 6);
    for c in payload.bytes() {
        let v = match c {
            48..=87 => c - 48,
            96..=119 => c - 56,
            _ => return None,
        };
        bits.extend((0..6).rev().map(|i| (v >> i) & 1 == 1));
    }
    bits.truncate(bits.len().checked_sub(fill.into())?);
    Some(bits)
}

fn checksum(body: &str) -> u8 {
    body.bytes().fold(0, |a, b| a ^ b)
}

/// Ein oder mehrere Sätze; lange Nachrichten wie Typ 5 brauchen zwei.
/// `seq_id` verbindet die Teile einer mehrteiligen Nachricht.
pub fn sentences(bits: &[bool], channel: char, seq_id: u8) -> Vec<String> {
    let (payload, fill) = armor(bits);
    let parts: Vec<&str> =
        payload.as_bytes().chunks(MAX_CHARS).map(|c| std::str::from_utf8(c).expect("nur ASCII")).collect();
    let total = parts.len();
    let seq = if total > 1 { (seq_id % 10).to_string() } else { String::new() };
    parts
        .iter()
        .enumerate()
        .map(|(i, part)| {
            let f = if i + 1 == total { fill } else { 0 };
            let body = format!("AIVDM,{total},{},{seq},{channel},{part},{f}", i + 1);
            format!("!{body}*{:02X}", checksum(&body))
        })
        .collect()
}

/// Liest einen einteiligen Satz und prüft die Prüfsumme.
#[cfg(test)]
pub fn parse(line: &str) -> Option<Vec<bool>> {
    let (body, cs) = line.strip_prefix('!')?.split_once('*')?;
    if u8::from_str_radix(cs.trim(), 16).ok()? != checksum(body) {
        return None;
    }
    let f: Vec<&str> = body.split(',').collect();
    if f.len() != 7 || f[1] != "1" {
        return None;
    }
    unarmor(f[5], f[6].parse().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::message::AisMessage;
    use crate::ais::message::tests::static_data;

    #[test]
    fn echter_satz_wird_gelesen() {
        // Beispiel aus der AIVDM/AIVDO-Protokollbeschreibung von Eric S. Raymond.
        let bits = parse("!AIVDM,1,1,,B,177KQJ5000G?tO`K>RA1wUbN0TKH,0*5C").expect("gültiger Satz");
        let AisMessage::Position(p) = AisMessage::decode(&bits).unwrap() else { panic!() };
        assert_eq!(p.mmsi, 477_553_000);
        assert_eq!(p.nav_status, 5);
        assert!((p.lat.unwrap() - 47.582833).abs() < 1e-5);
        assert!((p.lon.unwrap() - -122.345832).abs() < 1e-5);
        assert_eq!(p.heading, Some(181));
    }

    #[test]
    fn eigene_saetze_sind_gueltig_und_kurz_genug() {
        let bits = static_data().encode();
        let s = sentences(&bits, 'A', 3);
        assert_eq!(s.len(), 2);
        assert!(s.iter().all(|l| l.len() <= 82 && l.starts_with("!AIVDM,2,")));
        assert!(s[1].contains(",2,") && s[1].ends_with(&format!("*{:02X}", checksum(&s[1][1..s[1].len() - 3]))));

        let single = sentences(&bits[..168], 'B', 0);
        assert_eq!(parse(&single[0]).unwrap(), bits[..168]);
    }

    #[test]
    fn falsche_pruefsumme_wird_abgelehnt() {
        assert!(parse("!AIVDM,1,1,,B,177KQJ5000G?tO`K>RA1wUbN0TKH,0*5D").is_none());
    }
}
