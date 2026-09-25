mod ais;
mod dsp;
mod server;
mod source;
mod store;

use ais::{AisMessage, AisReceiver, ReceivedFrame, nmea};
use axum::body::Bytes;
use clap::Parser;
use dsp::{Detector, SpectrumEstimator};
use rustfft::num_complex::Complex32;
use source::{IqSource, SimulatedSource};
use std::net::UdpSocket;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use store::Store;
use tokio::sync::broadcast;

/// Echtzeit-Spektrumüberwachung mit AIS-Empfang und Web-Oberfläche.
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
    /// SQLite-Datei für Nachrichten und Schiffe (`:memory:` für flüchtig).
    #[arg(long, default_value = "ais.db")]
    db: String,
    /// Sendet jede Nachricht als NMEA per UDP, z. B. an OpenCPN auf 127.0.0.1:10110.
    #[arg(long)]
    nmea_udp: Option<String>,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let source = SimulatedSource::new(args.seed);
    let estimator = SpectrumEstimator::new(args.fft_size);
    let receiver = AisReceiver::new(source.sample_rate(), source.center_freq());

    let meta = api::SpectrumMeta {
        center_hz: source.center_freq(),
        sample_rate: source.sample_rate(),
        fft_size: estimator.size(),
        rbw_hz: estimator.rbw(source.sample_rate()),
        fps: args.fps,
        detector: format!("{:?}", args.detector),
    };
    let store = Arc::new(Mutex::new(Store::open(&args.db).expect("Datenbank lässt sich nicht öffnen")));
    let udp = args.nmea_udp.map(|target| {
        let socket = UdpSocket::bind("0.0.0.0:0").expect("UDP-Socket");
        socket.connect(&target).expect("ungültiges NMEA-Ziel");
        socket
    });

    let (tx, _) = broadcast::channel::<Bytes>(16);
    let (ais_tx, _) = broadcast::channel(256);
    let (frame_tx, frame_rx) = mpsc::channel();

    let producer = tx.clone();
    let (fps, detector) = (args.fps, args.detector);
    std::thread::spawn(move || acquisition_loop(source, estimator, receiver, fps, detector, producer, frame_tx));
    let (events, db) = (ais_tx.clone(), store.clone());
    std::thread::spawn(move || ais_pipeline(frame_rx, db, events, udp));

    let app = server::router(server::AppState {
        frames: tx,
        meta_json: serde_json::to_string(&meta).expect("serialisierbar").into(),
        ais_events: ais_tx,
        store,
    });
    let listener = tokio::net::TcpListener::bind(&args.bind).await.expect("Adresse ist belegt");
    println!("Spektrum und AIS laufen auf http://{}", args.bind);
    axum::serve(listener, app).await.expect("Server-Fehler");
}

/// Liest pro Frame genau die Samples, die in 1/fps Sekunden anfallen, damit
/// die Anzeige in Echtzeit läuft und keine Signalzeit verloren geht. Derselbe
/// Block geht an die Spektrumschätzung und an den AIS-Empfänger.
fn acquisition_loop(
    mut source: impl IqSource,
    mut estimator: SpectrumEstimator,
    mut receiver: AisReceiver,
    fps: f64,
    detector: Detector,
    tx: broadcast::Sender<Bytes>,
    frames: mpsc::Sender<ReceivedFrame>,
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
        for frame in receiver.process(&buf) {
            if frames.send(frame).is_err() {
                return;
            }
        }

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

/// Dekodiert empfangene Rahmen, speichert sie und verteilt sie weiter. Läuft
/// in eigenem Thread, damit Plattenzugriffe die Erfassung nicht aufhalten.
fn ais_pipeline(
    frames: mpsc::Receiver<ReceivedFrame>,
    store: Arc<Mutex<Store>>,
    events: broadcast::Sender<Arc<str>>,
    udp: Option<UdpSocket>,
) {
    let mut seq_id = 0u8;
    for frame in frames {
        let Some(mmsi) = ais::message::peek_mmsi(&frame.payload) else { continue };
        let decoded = AisMessage::decode(&frame.payload).ok();
        let nmea = nmea::sentences(&frame.payload, frame.channel.letter(), seq_id);
        if nmea.len() > 1 {
            seq_id = (seq_id + 1) % 10;
        }
        let msg_type = frame.payload.iter().take(6).fold(0u8, |a, &b| a << 1 | u8::from(b));
        let ts_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let message = api::MessageLog { ts_ms, channel: frame.channel.letter(), mmsi, msg_type, nmea };

        if let Some(socket) = &udp {
            for line in &message.nmea {
                let _ = socket.send(format!("{line}\r\n").as_bytes());
            }
        }
        let vessel = match store.lock().expect("Datenbank-Mutex vergiftet").record(&message, decoded.as_ref()) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("Speichern fehlgeschlagen: {e}");
                continue;
            }
        };
        let event = api::AisEvent { message, vessel };
        let _ = events.send(serde_json::to_string(&event).expect("serialisierbar").into());
    }
}
