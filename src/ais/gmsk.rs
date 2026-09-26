//! GMSK-Modulation, wie AIS sie nutzt: 9600 Bit/s, Modulationsindex 0,5
//! (±2,4 kHz Hub), Gaußfilter mit BT = 0,4.

pub const BAUD: f64 = 9600.0;
pub const DEVIATION_HZ: f64 = 2400.0;
const BT: f64 = 0.4;

/// Momentanfrequenz in Hz pro Sample, relativ zur Kanalmitte.
///
/// Jeder Pegel trägt einen Rechteckpuls von einer Bitdauer bei, gefaltet mit
/// dem Gaußfilter. Das ergibt die Differenz zweier Normalverteilungen. Der
/// Puls ist für jedes Bit gleich und nach zwei Bitdauern abgeklungen; er wird
/// deshalb einmal über fünf Bitdauern tabelliert und dann nur noch aufaddiert.
pub fn frequency(levels: &[bool], samples_per_bit: usize) -> Vec<f32> {
    let sps = samples_per_bit;
    let sigma = 2f64.ln().sqrt() / (std::f64::consts::TAU * BT); // in Bitdauern
    // Stützstelle m liegt (m + 0,5)/sps − 2,5 Bitdauern nach der Bitmitte.
    let pulse: Vec<f32> = (0..5 * sps)
        .map(|m| {
            let t = (m as f64 + 0.5) / sps as f64 - 2.5;
            ((phi((t + 0.5) / sigma) - phi((t - 0.5) / sigma)) * DEVIATION_HZ) as f32
        })
        .collect();

    let len = levels.len() * sps;
    let mut freq = vec![0.0f32; len];
    for (k, &level) in levels.iter().enumerate() {
        let sign = if level { 1.0 } else { -1.0 };
        // Der Puls von Bit k beginnt zwei Bitdauern vor dessen Anfang.
        let start = k as isize * sps as isize - 2 * sps as isize;
        for (m, p) in pulse.iter().enumerate() {
            let n = start + m as isize;
            if (0..len as isize).contains(&n) {
                freq[n as usize] += sign * p;
            }
        }
    }
    freq
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
