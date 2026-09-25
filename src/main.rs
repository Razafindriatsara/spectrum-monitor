// Wird erst mit der AIS-Pipeline in main verdrahtet, bis dahin nur in Tests genutzt.
#[allow(dead_code, unused_imports)]
mod ais;
mod dsp;
mod server;
mod source;

use axum::body::Bytes;
use clap::Parser;
use dsp::{Detector, SpectrumEstimator};
use rustfft::num_complex::Complex32;
use source::{IqSource, SimulatedSource};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Echtzeit-Spektrumüberwachung mit Web-Oberfläche.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// Adresse, auf der der Server lauscht.
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: String,
    /// FFT-Größe (Zweierpotenz). Größer heißt feinere Auflösung.
    #[arg(long, default_value_t = 2048)]
    fft_size: usize,
    /// Aktualisierungen pro Sekunde.
    #[arg(long, default_value_t = 25.0)]
    fps: f64,
    /// Detektor für die Segmente eines Frames.
    #[arg(long, value_enum, default_value_t = Detector::Peak)]
    detector: Detector,
    /// Startwert des Simulators, für reproduzierbare Läufe.
    #[arg(long, default_value_t = 1)]
    seed: u64,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let source = SimulatedSource::new(args.seed);
    let estimator = SpectrumEstimator::new(args.fft_size);

    let meta = api::SpectrumMeta {
        center_hz: source.center_freq(),
        sample_rate: source.sample_rate(),
        fft_size: estimator.size(),
        rbw_hz: estimator.rbw(source.sample_rate()),
        fps: args.fps,
        detector: format!("{:?}", args.detector),
    };
    let meta_json = serde_json::to_string(&meta).expect("serialisierbar");

    let (tx, _) = broadcast::channel::<Bytes>(16);
    let producer = tx.clone();
    let (fps, detector) = (args.fps, args.detector);
    std::thread::spawn(move || acquisition_loop(source, estimator, fps, detector, producer));

    let app = server::router(server::AppState { frames: tx, meta_json });
    let listener = tokio::net::TcpListener::bind(&args.bind).await.expect("Adresse ist belegt");
    println!("Spektrum läuft auf http://{}", args.bind);
    axum::serve(listener, app).await.expect("Server-Fehler");
}

/// Liest pro Frame genau die Samples, die in 1/fps Sekunden anfallen, damit
/// die Anzeige in Echtzeit läuft und keine Signalzeit verloren geht.
fn acquisition_loop(
    mut source: impl IqSource,
    mut estimator: SpectrumEstimator,
    fps: f64,
    detector: Detector,
    tx: broadcast::Sender<Bytes>,
) {
    let per_frame = (source.sample_rate() / fps) as usize;
    let mut buf = vec![Complex32::ZERO; per_frame];
    let period = Duration::from_secs_f64(1.0 / fps);
    let mut next = Instant::now();
    let mut warned = false;

    loop {
        source.read(&mut buf);
        let spectrum = estimator.process(&buf, detector);
        let bytes: Vec<u8> = spectrum.iter().flat_map(|v| v.to_le_bytes()).collect();
        // Fehler heißt nur: gerade ist kein Client verbunden.
        let _ = tx.send(Bytes::from(bytes));

        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            if !warned {
                eprintln!("Warnung: Verarbeitung langsamer als Echtzeit. Mit --release bauen oder --fps senken.");
                warned = true;
            }
            next = now;
        }
    }
}
