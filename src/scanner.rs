//! Signalüberwachung: findet Sender im Spektrum, verfolgt sie, klassifiziert
//! ihre Modulation und meldet, wenn einer auftaucht, seine Klasse wechselt
//! oder verschwindet. Läuft in eigenem Thread, damit die Klassifikation die
//! Erfassung nicht aufhält.

use api::{Signal, SignalEvent, SignalEventKind};
use rustfft::num_complex::Complex32;
use spectrum_monitor::classify::{self, Class, Classifier};
use spectrum_monitor::detect::{CfarConfig, TrackEvent, Tracker, cfar};
use spectrum_monitor::dsp::{Detector, SpectrumEstimator};
use std::collections::HashMap;

/// Neue Signale werden bei jedem Treffer klassifiziert, bis so viele Stimmen
/// zusammen sind; danach nur noch einmal pro Sekunde.
const EARLY_VOTES: u32 = 5;
/// Gemeldet wird ein Signal erst, wenn seine Klasse auf so vielen Stimmen beruht.
const MIN_VOTES: u32 = 5;
const REVOTE_S: f64 = 1.0;
/// So oft pro Sekunde geht eine Übersicht aller Signale hinaus.
const SNAPSHOT_HZ: f64 = 4.0;

/// Was ein Frame an Ausgaben erzeugt.
#[derive(Default)]
pub struct Output {
    pub events: Vec<SignalEvent>,
    pub snapshot: Option<Vec<Signal>>,
}

pub struct Scanner {
    estimator: SpectrumEstimator,
    cfar: CfarConfig,
    tracker: Tracker,
    classifier: Classifier,
    sample_rate: f64,
    center_hz: f64,
    frame_s: f64,
    /// Signalzeit in Sekunden seit dem Start.
    t: f64,
    next_snapshot: f64,
    /// Gemeldete Signale mit ihrem zuletzt gemeldeten Stand.
    announced: HashMap<u64, Signal>,
}

impl Scanner {
    pub fn new(
        classifier: Classifier,
        sample_rate: f64,
        center_hz: f64,
        fft_size: usize,
        frame_s: f64,
        cfar: CfarConfig,
    ) -> Self {
        Self {
            estimator: SpectrumEstimator::new(fft_size),
            cfar,
            tracker: Tracker::new(Class::ALL.len(), frame_s),
            classifier,
            sample_rate,
            center_hz,
            frame_s,
            t: 0.0,
            next_snapshot: 0.0,
            announced: HashMap::new(),
        }
    }

    /// Verarbeitet einen IQ-Block. `now_ms` ist die Uhrzeit für die Ereignisse.
    pub fn process(&mut self, block: &[Complex32], now_ms: i64) -> Output {
        let bin_hz = self.sample_rate / self.estimator.size() as f64;
        // Spitzendetektor: kurze Bursts wie AIS sollen nicht im Mittel untergehen.
        let spectrum = self.estimator.process(block, Detector::Peak);
        let detections = cfar(&spectrum, bin_hz, &self.cfar);
        let (ids, events) = self.tracker.update(self.t, &detections, bin_hz);

        for (d, id) in detections.iter().zip(ids) {
            let t = self.t;
            let Some(track) = self.tracker.get_mut(id) else { continue };
            // Ab dem zweiten Treffer: Einzeltreffer sind meist Springer oder Rauschen.
            let due = track.hits >= 2 && (track.classified < EARLY_VOTES || t - track.last_classified >= REVOTE_S);
            if !due {
                continue;
            }
            let Some(window) = classify::extract(block, self.sample_rate, d.offset_hz) else { continue };
            match self.classifier.classify(&classify::features(&window)) {
                // Hält das Netz das Fenster für Rauschen, sagt es nichts über
                // die Modulation, etwa wenn nur das Ende eines Bursts in den
                // Frame ragt. Das zählt als Enthaltung, nicht als Stimme; ein
                // Fehlalarm sammelt so nie genug Stimmen, um gemeldet zu werden.
                Ok(probs) if probs[Class::Noise.index()] >= 0.5 => {}
                Ok(probs) => {
                    track.votes.iter_mut().zip(probs).for_each(|(v, p)| *v += p);
                    track.classified += 1;
                    track.last_classified = t;
                }
                Err(e) => eprintln!("Klassifikation fehlgeschlagen: {e}"),
            }
        }

        let mut out = Output::default();
        for e in events {
            if let TrackEvent::Lost(id) = e
                && let Some(signal) = self.announced.remove(&id)
            {
                out.events.push(SignalEvent { ts_ms: now_ms, kind: SignalEventKind::Lost, signal });
            }
        }

        // Bestätigte Signale melden, sobald ihre Klasse auf genug Stimmen beruht
        // und kein Rauschen ist.
        let current: Vec<Signal> = self
            .tracker
            .tracks()
            .iter()
            .filter(|tr| tr.confirmed && tr.classified >= MIN_VOTES)
            .filter_map(|tr| {
                let (class, confidence) = tr.class()?;
                Some(Signal {
                    id: tr.id,
                    center_hz: self.center_hz + tr.offset_hz,
                    bandwidth_hz: tr.bandwidth_hz,
                    peak_dbfs: tr.peak_db,
                    snr_db: tr.snr_db,
                    class: Class::ALL[class].label().to_string(),
                    confidence,
                    first_seen_ms: self.wall_ms(now_ms, tr.first_seen),
                    last_seen_ms: self.wall_ms(now_ms, tr.last_seen),
                })
            })
            .collect();
        for signal in &current {
            let kind = match self.announced.get(&signal.id) {
                None if signal.class != Class::Noise.label() => SignalEventKind::Appeared,
                Some(prev) if prev.class != signal.class => SignalEventKind::Reclassified,
                _ => {
                    if let Some(prev) = self.announced.get_mut(&signal.id) {
                        *prev = signal.clone();
                    }
                    continue;
                }
            };
            self.announced.insert(signal.id, signal.clone());
            out.events.push(SignalEvent { ts_ms: now_ms, kind, signal: signal.clone() });
        }

        if self.t >= self.next_snapshot {
            self.next_snapshot = self.t + 1.0 / SNAPSHOT_HZ;
            let mut signals: Vec<Signal> = current.into_iter().filter(|s| self.announced.contains_key(&s.id)).collect();
            signals.sort_by(|a, b| a.center_hz.total_cmp(&b.center_hz));
            out.snapshot = Some(signals);
        }
        self.t += self.frame_s;
        out
    }

    /// Uhrzeit zu einem Zeitpunkt in Signalzeit.
    fn wall_ms(&self, now_ms: i64, t: f64) -> i64 {
        now_ms - ((self.t - t) * 1000.0) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spectrum_monitor::source::{IqSource, SimulatedSource};

    /// Das mitgelieferte Modell, wie es auch der Server einbettet.
    fn classifier() -> Classifier {
        Classifier::from_bytes(include_bytes!("../models/modulation.onnx")).expect("Modell lädt")
    }

    #[test]
    fn simulator_signale_werden_erkannt_und_richtig_klassifiziert() {
        let mut src = SimulatedSource::new(5);
        let fs = src.sample_rate();
        let mut scanner = Scanner::new(classifier(), fs, src.center_freq(), 2048, 0.04, CfarConfig::default());
        let mut buf = vec![Complex32::ZERO; 96_000];
        let mut last = Vec::new();
        let mut events = Vec::new();
        // 15 s Signalzeit: genug, damit auch beide AIS-Kanäle bestätigt sind.
        for frame in 0..375 {
            src.read(&mut buf);
            let out = scanner.process(&buf, frame * 40);
            events.extend(out.events);
            if let Some(s) = out.snapshot {
                last = s;
            }
        }
        let found: Vec<(i64, &str)> =
            last.iter().map(|s| (((s.center_hz - 162e6) / 1000.0).round() as i64, s.class.as_str())).collect();
        assert_eq!(
            found,
            vec![(-900, "AM"), (-600, "Träger"), (-450, "QPSK"), (-300, "FM"), (-25, "GMSK"), (25, "GMSK"),]
        );
        assert!(events.iter().all(|e| e.kind != SignalEventKind::Lost), "{events:#?}");
    }
}
