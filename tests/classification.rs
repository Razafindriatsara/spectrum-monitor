//! Prüft das mitgelieferte Modell `models/modulation.onnx`: Es muss auf frisch
//! erzeugten Beispielen jede Klasse zuverlässig erkennen, und die Lage des
//! Signals im Ausschnitt darf keine Rolle spielen.

use rand::SeedableRng;
use rand::rngs::StdRng;
use rand_distr::{Distribution, Normal};
use rustfft::num_complex::Complex32;
use spectrum_monitor::classify::{self, Class, Classifier};
use spectrum_monitor::dsp::Oscillator;

fn classifier() -> Classifier {
    Classifier::from_bytes(include_bytes!("../models/modulation.onnx")).expect("Modell lädt")
}

fn best(p: &[f32]) -> Class {
    Class::ALL[p.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0]
}

#[test]
fn jede_klasse_wird_meist_erkannt() {
    let c = classifier();
    // Andere Startwerte als Training (1) und Validierung.
    let mut rng = StdRng::seed_from_u64(4242);
    for class in Class::ALL {
        let n = 40;
        let hits =
            (0..n).filter(|_| best(&c.classify(&classify::synthesize(class, &mut rng)).unwrap()) == class).count();
        assert!(hits * 100 >= n * 85, "{}: nur {hits} von {n} erkannt", class.label());
    }
}

#[test]
fn traeger_wird_unabhaengig_von_der_ablage_erkannt() {
    let c = classifier();
    let fs = 2.4e6;
    for snr_db in [20.0, 40.0, 60.0] {
        // Genau getroffen, knapp daneben, weit daneben.
        for (seed, offset) in [(1, 0.0), (2, 30.0), (3, 300.0), (4, -900.0)] {
            let mut rng = StdRng::seed_from_u64(seed);
            let sigma = (fs / (2.0 * 48_000.0 * 10f64.powf(snr_db / 10.0))).sqrt() as f32;
            let noise = Normal::new(0.0, sigma).unwrap();
            let mut osc = Oscillator::new(offset, fs);
            let block: Vec<Complex32> = (0..96_000)
                .map(|_| osc.sample() + Complex32::new(noise.sample(&mut rng), noise.sample(&mut rng)))
                .collect();
            let p = c.classify(&classify::features(&classify::extract(&block, fs, 0.0).unwrap())).unwrap();
            assert_eq!(best(&p), Class::Carrier, "{snr_db} dB, {offset} Hz: {p:?}");
        }
    }
}
