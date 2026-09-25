//! Bitfelder der AIS-Nutzdaten: höchstwertiges Bit zuerst, vorzeichenbehaftete
//! Felder im Zweierkomplement, Text in 6-Bit-ASCII.

#[derive(Default)]
pub struct BitWriter {
    bits: Vec<bool>,
}

impl BitWriter {
    pub fn uint(&mut self, v: u64, n: usize) {
        for i in (0..n).rev() {
            self.bits.push((v >> i) & 1 == 1);
        }
    }

    pub fn int(&mut self, v: i64, n: usize) {
        self.uint(v as u64 & ((1 << n) - 1), n);
    }

    /// Schreibt genau `chars` Zeichen und füllt mit `@` (Wert 0) auf.
    pub fn text(&mut self, s: &str, chars: usize) {
        for c in s.chars().chain(std::iter::repeat('@')).take(chars) {
            self.uint(sixbit(c).into(), 6);
        }
    }

    pub fn into_bits(self) -> Vec<bool> {
        self.bits
    }
}

pub struct BitReader<'a> {
    bits: &'a [bool],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(bits: &'a [bool]) -> Self {
        Self { bits, pos: 0 }
    }

    pub fn uint(&mut self, n: usize) -> Option<u64> {
        let field = self.bits.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(field.iter().fold(0, |acc, &b| acc << 1 | u64::from(b)))
    }

    pub fn int(&mut self, n: usize) -> Option<i64> {
        let v = self.uint(n)?;
        let sign = 1 << (n - 1);
        Some(if v & sign != 0 { v as i64 - (1 << n) } else { v as i64 })
    }

    /// Liest `chars` Zeichen; Füllzeichen `@` und Leerzeichen am Ende entfallen.
    pub fn text(&mut self, chars: usize) -> Option<String> {
        let mut s = String::with_capacity(chars);
        for _ in 0..chars {
            let v = self.uint(6)? as u8;
            s.push(if v < 32 { (v + 64) as char } else { v as char });
        }
        Some(s.trim_end_matches(['@', ' ']).to_string())
    }
}

/// Zeichen nach 6-Bit-ASCII: `@A…Z[\]^_` sind 0–31, Leerzeichen bis `?` sind 32–63.
fn sixbit(c: char) -> u8 {
    match c.to_ascii_uppercase() {
        c @ '@'..='_' => c as u8 - 64,
        c @ ' '..='?' => c as u8,
        _ => b'?',
    }
}

/// Packt Bits in Bytes, höchstwertiges Bit zuerst, und füllt mit Nullen auf.
pub fn to_bytes(bits: &[bool]) -> Vec<u8> {
    bits.chunks(8).map(|c| c.iter().enumerate().fold(0u8, |acc, (i, &b)| acc | u8::from(b) << (7 - i))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn felder_ueberstehen_hin_und_rueckweg() {
        let mut w = BitWriter::default();
        w.uint(5, 6);
        w.int(-12345, 28);
        w.text("KIEL 1", 8);
        let bits = w.into_bits();
        assert_eq!(bits.len(), 6 + 28 + 48);
        let mut r = BitReader::new(&bits);
        assert_eq!(r.uint(6), Some(5));
        assert_eq!(r.int(28), Some(-12345));
        assert_eq!(r.text(8).as_deref(), Some("KIEL 1"));
        assert_eq!(r.uint(1), None);
    }
}
