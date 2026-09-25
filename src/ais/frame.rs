//! Sicherungsschicht: HDLC-Rahmen mit CRC-16, Bit-Stuffing und NRZI.
//!
//! Aufbau eines Rahmens auf der Luftschnittstelle:
//! Trainingsfolge (24 Bit `0101…`), Flag `0x7E`, Nutzdaten und FCS mit
//! Bit-Stuffing, Flag `0x7E`. Jedes Byte geht mit dem niederwertigsten Bit
//! zuerst hinaus. NRZI: eine 0 wechselt den Pegel, eine 1 hält ihn.

use super::bits::to_bytes;

const FLAG: u8 = 0x7E;
const TRAINING_BITS: usize = 24;
/// CRC über Daten und FCS ergibt bei fehlerfreier Übertragung immer diesen Rest.
const CRC_GOOD: u16 = 0xF0B8;
/// Längster Rahmen: fünf Zeitschlitze zu 256 Bit.
const MAX_FRAME_BITS: usize = 5 * 256;

/// CRC-16/X-25 (HDLC) über Bits in Sendereihenfolge.
fn crc16(bits: impl IntoIterator<Item = bool>) -> u16 {
    bits.into_iter().fold(0xFFFF, |crc, b| {
        let feedback = (crc ^ u16::from(b)) & 1;
        let crc = crc >> 1;
        if feedback == 1 { crc ^ 0x8408 } else { crc }
    })
}

fn lsb_first(byte: u8) -> impl Iterator<Item = bool> {
    (0..8).map(move |i| (byte >> i) & 1 == 1)
}

/// Bitfolge vor der NRZI-Kodierung, Nutzdaten werden auf volle Bytes aufgefüllt.
pub fn build(payload: &[bool]) -> Vec<bool> {
    let data: Vec<bool> = to_bytes(payload).into_iter().flat_map(lsb_first).collect();
    let fcs = !crc16(data.iter().copied());
    let mut out: Vec<bool> = (0..TRAINING_BITS).map(|i| i % 2 == 1).collect();
    out.extend(lsb_first(FLAG));

    let mut ones = 0;
    for b in data.into_iter().chain((0..16).map(|i| (fcs >> i) & 1 == 1)) {
        out.push(b);
        ones = if b { ones + 1 } else { 0 };
        if ones == 5 {
            out.push(false);
            ones = 0;
        }
    }
    out.extend(lsb_first(FLAG));
    out
}

/// NRZI-Kodierung zu Pegeln (`true` = positive Frequenzablage).
pub fn nrzi(bits: &[bool]) -> Vec<bool> {
    let mut level = false;
    bits.iter()
        .map(|&b| {
            if !b {
                level = !level;
            }
            level
        })
        .collect()
}

/// Sucht in einem Strom dekodierter Bits nach Rahmen und prüft ihre CRC.
#[derive(Default)]
pub struct Deframer {
    shift: u8,
    in_frame: bool,
    ones: u32,
    data: Vec<bool>,
}

impl Deframer {
    /// Nimmt ein Bit nach der NRZI-Dekodierung. Liefert die Nutzdaten, sobald
    /// ein vollständiger Rahmen mit gültiger CRC zu Ende ist.
    pub fn push(&mut self, bit: bool) -> Option<Vec<bool>> {
        self.shift = self.shift >> 1 | u8::from(bit) << 7;
        if self.shift == FLAG {
            let frame = if self.in_frame { self.finish() } else { None };
            // Das schließende Flag kann gleich den nächsten Rahmen eröffnen.
            self.in_frame = true;
            self.ones = 0;
            self.data.clear();
            return frame;
        }
        if !self.in_frame {
            return None;
        }
        if bit {
            self.ones += 1;
            if self.ones > 6 {
                self.in_frame = false; // Abbruchsequenz oder Rauschen
                return None;
            }
            self.data.push(true);
        } else if self.ones == 5 {
            self.ones = 0; // eingefügtes Stopfbit
        } else {
            self.ones = 0;
            self.data.push(false);
        }
        if self.data.len() > MAX_FRAME_BITS {
            self.in_frame = false;
        }
        None
    }

    fn finish(&mut self) -> Option<Vec<bool>> {
        // Die ersten sieben Bits des Flags sind schon als Daten gelandet.
        let len = self.data.len().checked_sub(7)?;
        let bits = &self.data[..len];
        if len < 8 * 8 || len % 8 != 0 || crc16(bits.iter().copied()) != CRC_GOOD {
            return None;
        }
        // Bytes kamen LSB zuerst, die Nutzdaten sind MSB zuerst gepackt.
        let payload = bits[..len - 16].chunks(8).flat_map(|byte| byte.iter().rev().copied()).collect();
        Some(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ais::message::tests::{position, static_data};

    #[test]
    fn crc_entspricht_x25_pruefwert() {
        let bits = b"123456789".iter().flat_map(|&b| lsb_first(b));
        assert_eq!(!crc16(bits), 0x906E);
    }

    fn decode_levels(levels: &[bool]) -> Vec<Vec<bool>> {
        let mut d = Deframer::default();
        let mut prev = false;
        levels
            .iter()
            .filter_map(|&l| {
                let bit = l == prev;
                prev = l;
                d.push(bit)
            })
            .collect()
    }

    #[test]
    fn rahmen_ueberlebt_hin_und_rueckweg() {
        for msg in [position(), static_data()] {
            let payload = msg.encode();
            let frames = decode_levels(&nrzi(&build(&payload)));
            assert_eq!(frames, vec![payload]);
        }
    }

    #[test]
    fn stuffing_verhindert_flags_in_den_daten() {
        let payload = vec![true; 168];
        let bits = build(&payload);
        let body = &bits[TRAINING_BITS + 8..bits.len() - 8];
        assert!(body.windows(6).all(|w| !w.iter().all(|&b| b)));
        assert_eq!(decode_levels(&nrzi(&bits)), vec![payload]);
    }

    #[test]
    fn gekipptes_bit_wird_verworfen() {
        let mut levels = nrzi(&build(&position().encode()));
        let i = levels.len() / 2;
        // Pegel ab hier invertieren kippt in NRZI genau ein Bit.
        levels[i..].iter_mut().for_each(|l| *l = !*l);
        assert!(decode_levels(&levels).is_empty());
    }
}
