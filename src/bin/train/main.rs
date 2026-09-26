//! Trainiert den Modulationsklassifikator und schreibt ihn als ONNX-Modell.
//!
//! ```text
//! cargo run --release --features train --bin train
//! ```
//!
//! Die Beispiele entstehen mit [`classify::synthesize`], also mit denselben
//! Generatoren und derselben Merkmalsextraktion wie im Betrieb. Nach dem
//! Training lädt der Trainer das exportierte Modell mit tract und prüft, dass
//! es dieselben Wahrscheinlichkeiten liefert wie das Netz in candle.

// Der MSVC-Linker meldet, dass er train.lib und train.exp anlegt, weil eine
// Abhängigkeit Symbole exportiert. Das ist harmlos.
#![allow(linker_messages)]

mod model;
mod onnx;

use candle_core::{D, DType, Device, Tensor};
use candle_nn::{AdamW, Optimizer, ParamsAdamW, VarBuilder, VarMap, loss, ops};
use clap::Parser;
use model::Net;
use rand::seq::SliceRandom;
use rand::{SeedableRng, rngs::StdRng};
use spectrum_monitor::classify::{self, CHANNELS, Class, Classifier, WINDOW};
use std::time::Instant;

#[derive(Parser)]
struct Args {
    /// Trainingsbeispiele pro Klasse.
    #[arg(long, default_value_t = 1500)]
    train: usize,
    /// Validierungsbeispiele pro Klasse, getrennt erzeugt.
    #[arg(long, default_value_t = 300)]
    val: usize,
    #[arg(long, default_value_t = 20)]
    epochs: usize,
    #[arg(long, default_value_t = 64)]
    batch: usize,
    #[arg(long, default_value_t = 5e-3)]
    lr: f64,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    #[arg(long, default_value = "models/modulation.onnx")]
    out: String,
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Merkmale und Klassennummern, Beispiel für Beispiel hintereinander.
struct Dataset {
    x: Vec<f32>,
    y: Vec<u32>,
}

impl Dataset {
    /// Erzeugt `per_class` Beispiele pro Klasse auf allen Kernen. Jedes Beispiel
    /// hat einen eigenen Startwert, das Ergebnis hängt also nicht von der
    /// Zahl der Threads ab.
    fn generate(per_class: usize, seed: u64) -> Self {
        let n = per_class * Class::ALL.len();
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let chunk = n.div_ceil(threads);
        let parts: Vec<Vec<f32>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..n)
                .step_by(chunk)
                .map(|start| {
                    s.spawn(move || {
                        (start..(start + chunk).min(n))
                            .flat_map(|i| {
                                let mut rng =
                                    StdRng::seed_from_u64(seed.wrapping_mul(1_000_003).wrapping_add(i as u64));
                                classify::synthesize(Class::ALL[i % Class::ALL.len()], &mut rng)
                            })
                            .collect()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().expect("Thread abgestürzt")).collect()
        });
        let y = (0..n).map(|i| (i % Class::ALL.len()) as u32).collect();
        Self { x: parts.concat(), y }
    }

    fn len(&self) -> usize {
        self.y.len()
    }

    fn sample(&self, i: usize) -> &[f32] {
        let size = CHANNELS * WINDOW;
        &self.x[i * size..(i + 1) * size]
    }

    fn batch(&self, idx: &[usize], dev: &Device) -> candle_core::Result<(Tensor, Tensor)> {
        let x: Vec<f32> = idx.iter().flat_map(|&i| self.sample(i).iter().copied()).collect();
        let y: Vec<u32> = idx.iter().map(|&i| self.y[i]).collect();
        Ok((Tensor::from_vec(x, (idx.len(), CHANNELS, WINDOW), dev)?, Tensor::from_vec(y, idx.len(), dev)?))
    }
}

/// Wahrscheinlichkeiten des Netzes für alle Beispiele, in Stapeln.
fn predict(net: &Net, data: &Dataset, dev: &Device) -> candle_core::Result<Vec<Vec<f32>>> {
    let idx: Vec<usize> = (0..data.len()).collect();
    let mut out = Vec::with_capacity(data.len());
    for chunk in idx.chunks(256) {
        let (x, _) = data.batch(chunk, dev)?;
        out.extend(ops::softmax(&net.forward(&x, false)?, D::Minus1)?.to_vec2::<f32>()?);
    }
    Ok(out)
}

fn argmax(p: &[f32]) -> usize {
    p.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(0, |(i, _)| i)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let dev = Device::Cpu;

    let t = Instant::now();
    let train = Dataset::generate(args.train, args.seed);
    // Anderer Startwertbereich, damit sich Training und Validierung nicht überschneiden.
    let val = Dataset::generate(args.val, args.seed.wrapping_add(0x5EED));
    println!(
        "{} Trainings- und {} Validierungsbeispiele in {:.1} s",
        train.len(),
        val.len(),
        t.elapsed().as_secs_f32()
    );

    let varmap = VarMap::new();
    let net = Net::new(VarBuilder::from_varmap(&varmap, DType::F32, &dev))?;
    let mut opt = AdamW::new(varmap.all_vars(), ParamsAdamW { lr: args.lr, weight_decay: 1e-4, ..Default::default() })?;
    let mut rng = StdRng::seed_from_u64(args.seed);
    let mut order: Vec<usize> = (0..train.len()).collect();

    for epoch in 0..args.epochs {
        let t = Instant::now();
        // Kosinusförmig abnehmende Lernrate.
        let progress = epoch as f64 / args.epochs as f64;
        opt.set_learning_rate(args.lr * 0.5 * (1.0 + (std::f64::consts::PI * progress).cos()));
        order.shuffle(&mut rng);
        let mut loss_sum = 0.0;
        let batches = order.chunks(args.batch);
        let n_batches = batches.len();
        for idx in batches {
            let (x, y) = train.batch(idx, &dev)?;
            let loss = loss::cross_entropy(&net.forward(&x, true)?, &y)?;
            opt.backward_step(&loss)?;
            loss_sum += loss.to_scalar::<f32>()?;
        }
        let probs = predict(&net, &val, &dev)?;
        let correct = probs.iter().zip(&val.y).filter(|(p, y)| argmax(p) == **y as usize).count();
        println!(
            "Epoche {:2}: Verlust {:.4}, Validierung {:.1} %, {:.1} s",
            epoch + 1,
            loss_sum / n_batches as f32,
            100.0 * correct as f32 / val.len() as f32,
            t.elapsed().as_secs_f32()
        );
    }

    // Verwechslungsmatrix auf den Validierungsdaten.
    let probs = predict(&net, &val, &dev)?;
    let k = Class::ALL.len();
    let mut confusion = vec![vec![0usize; k]; k];
    for (p, &y) in probs.iter().zip(&val.y) {
        confusion[y as usize][argmax(p)] += 1;
    }
    println!("\nVerwechslungsmatrix (Zeile: wahr, Spalte: erkannt)");
    print!("{:>10}", "");
    Class::ALL.iter().for_each(|c| print!("{:>9}", c.label()));
    println!();
    for (c, row) in Class::ALL.iter().zip(&confusion) {
        print!("{:>10}", c.label());
        row.iter().for_each(|n| print!("{n:>9}"));
        println!();
    }
    let accuracy = (0..k).map(|i| confusion[i][i]).sum::<usize>() as f32 / val.len() as f32;

    let classes = Class::ALL.iter().map(|c| c.label()).collect::<Vec<_>>().join(",");
    let bytes = onnx::export(
        &net.export_weights()?,
        &[
            ("classes", classes),
            ("input", format!("{CHANNELS}x{WINDOW} bei 48 kHz: Betrag, Momentanfrequenz/5 kHz, Spektrum")),
            ("validation_accuracy", format!("{accuracy:.4}")),
            ("seed", args.seed.to_string()),
        ],
    );

    // Das exportierte Modell muss in tract dasselbe liefern wie in candle.
    let classifier = Classifier::from_bytes(&bytes)?;
    let mut max_diff = 0f32;
    for i in (0..val.len()).step_by(val.len() / 60) {
        let tract = classifier.classify(val.sample(i))?;
        max_diff = tract.iter().zip(&probs[i]).map(|(a, b)| (a - b).abs()).fold(max_diff, f32::max);
    }
    println!("\nGrößte Abweichung tract gegenüber candle: {max_diff:.2e}");
    if max_diff > 1e-4 {
        return Err(format!("ONNX-Export weicht ab ({max_diff})").into());
    }

    if let Some(dir) = std::path::Path::new(&args.out).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&args.out, &bytes)?;
    println!("Genauigkeit {:.1} %, {} geschrieben ({} kB)", 100.0 * accuracy, args.out, bytes.len() / 1024);
    Ok(())
}
