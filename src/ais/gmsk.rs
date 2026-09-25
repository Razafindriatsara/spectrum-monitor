//! GMSK-Modulation, wie AIS sie nutzt: 9600 Bit/s, Modulationsindex 0,5
//! (±2,4 kHz Hub), Gaußfilter mit BT = 0,4.

pub const BAUD: f64 = 9600.0;
pub const DEVIATION_HZ: f64 = 2400.0;
const BT: f64 = 0.4;

/// Momentanfrequenz in Hz pro Sample, relativ zur Kanalmitte.
///
/// Jeder Pegel trägt einen Rechteckpuls von einer Bitdauer bei, gefaltet mit
/// dem Gaußfilter. Das ergibt die Differenz zweier Normalverteilungen.
pub fn frequency(levels: &[bool], samples_per_bit: usize) -> Vec<f32> {
    let sps = samples_per_bit as f64;
    let sigma = 2f64.ln().sqrt() / (std::f64::consts::TAU * BT); // in Bitdauern
    let pulse = |t: f64| phi((t + 0.5) / sigma) - phi((t - 0.5) / sigma);
    let a = |k: isize| match levels.get(k as usize) {
        Some(&l) if k >= 0 => {
            if l {
                1.0
            } else {
                -1.0
            }
        }
        _ => 0.0,
    };

    (0..levels.len() * samples_per_bit)
        .map(|n| {
            let t = (n as f64 + 0.5) / sps; // Zeit in Bitdauern
            let k0 = t.floor() as isize;
            // Der Puls ist nach zwei Bitdauern praktisch abgeklungen.
            let f: f64 = (k0 - 2..=k0 + 2).map(|k| a(k) * pulse(t - k as f64 - 0.5)).sum();
            (f * DEVIATION_HZ) as f32
        })
        .collect()
}

/// Verteilungsfunktion der Standardnormalverteilung.
fn phi(x: f64) -> f64 {
    0.5 * (1.0 + erf(x / std::f64::consts::SQRT_2))
}

/// Fehlerfunktion nach Abramowitz und Stegun 7.1.26, Fehler unter 1,5·10⁻⁷.
fn erf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.3275911 * x.abs());
    let poly = t * (0.254829592 + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
    (1.0 - poly * (-x * x).exp()).copysign(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dauerpegel_erreicht_vollen_hub() {
        let f = frequency(&[true; 8], 10);
        assert!((f[40] as f64 - DEVIATION_HZ).abs() < 1.0);
    }

    #[test]
    fn wechselfolge_bleibt_unter_dem_hub() {
        // Gaußfilter verschleift schnelle Wechsel: kein Bit erreicht den vollen Hub.
        let levels: Vec<bool> = (0..16).map(|i| i % 2 == 0).collect();
        let f = frequency(&levels, 10);
        let peak = f[40..120].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.5 * DEVIATION_HZ as f32 && peak < 0.9 * DEVIATION_HZ as f32, "{peak}");
    }
}
