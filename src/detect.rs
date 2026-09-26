//! Signaldetektion im Spektrum und Verfolgung über die Zeit.
//!
//! 1. [`cfar`]: CA-CFAR (Cell Averaging, Constant False Alarm Rate). Für jedes
//!    Bin schätzt der Mittelwert der Trainingszellen links und rechts das
//!    Rauschen; Schutzzellen dazwischen halten das Signal selbst aus der
//!    Schätzung heraus. Ein Bin gilt als belegt, wenn es die Schätzung um
//!    eine feste Schwelle in dB übersteigt. So passt sich die Schwelle an ein
//!    ungleichmäßiges Rauschen an, statt fest in dBFS zu liegen.
//! 2. Belegte Nachbarbins werden zu einer [`Detection`] zusammengefasst.
//! 3. Der [`Tracker`] ordnet Detektionen über Frames hinweg einander zu. Ein
//!    Signal gilt als bestätigt, wenn es entweder mehrere Frames am Stück da
//!    ist (Dauersender) oder in mehreren getrennten Bursts auf genau derselben
//!    Frequenz wiederkommt (etwa AIS). Ein Frequenzspringer erfüllt beides
//!    nicht: Er bleibt nur Sekundenbruchteile und trifft eine alte Frequenz
//!    höchstens zufällig wieder.

/// Einstellungen des CFAR-Detektors.
#[derive(Clone, Copy, Debug)]
pub struct CfarConfig {
    /// Schutzzellen auf jeder Seite des geprüften Bins.
    pub guard: usize,
    /// Trainingszellen auf jeder Seite, aus denen das Rauschen geschätzt wird.
    pub train: usize,
    /// Wie weit ein Bin über der Rauschschätzung liegen muss.
    pub threshold_db: f32,
    /// Lücken bis zu dieser Breite in Bins verbinden zwei Treffer zu einem Signal.
    pub max_gap: usize,
}

impl Default for CfarConfig {
    fn default() -> Self {
        Self { guard: 8, train: 24, threshold_db: 10.0, max_gap: 2 }
    }
}

/// Ein zusammenhängend belegter Frequenzbereich in einem Spektrum.
#[derive(Clone, Debug, PartialEq)]
pub struct Detection {
    /// Leistungsgewichteter Schwerpunkt, relativ zur Bandmitte.
    pub offset_hz: f64,
    pub bandwidth_hz: f64,
    pub peak_db: f32,
    /// Rauschschätzung am stärksten Bin.
    pub noise_db: f32,
    pub first_bin: usize,
    pub last_bin: usize,
}

impl Detection {
    pub fn snr_db(&self) -> f32 {
        self.peak_db - self.noise_db
    }
}

/// Findet Signale in einem zentrierten Spektrum in dB (Bin 0 ist die
/// niedrigste Frequenz). `bin_hz` ist der Abstand der Bins.
pub fn cfar(spectrum_db: &[f32], bin_hz: f64, cfg: &CfarConfig) -> Vec<Detection> {
    let n = spectrum_db.len();
    let lin: Vec<f64> = spectrum_db.iter().map(|&d| 10f64.powf(f64::from(d) / 10.0)).collect();
    // Präfixsummen: jede Fenstersumme in konstanter Zeit.
    let mut prefix = vec![0.0; n + 1];
    for (i, v) in lin.iter().enumerate() {
        prefix[i + 1] = prefix[i] + v;
    }
    let sum = |a: usize, b: usize| prefix[b] - prefix[a];
    let factor = 10f64.powf(f64::from(cfg.threshold_db) / 10.0);

    // Rauschschätzung pro Bin; am Rand zählt nur die Seite, die es gibt.
    let noise: Vec<f64> = (0..n)
        .map(|i| {
            let left = (i.saturating_sub(cfg.guard + cfg.train), i.saturating_sub(cfg.guard));
            let right = ((i + cfg.guard + 1).min(n), (i + cfg.guard + 1 + cfg.train).min(n));
            let cells = (left.1 - left.0) + (right.1 - right.0);
            (sum(left.0, left.1) + sum(right.0, right.1)) / cells.max(1) as f64
        })
        .collect();
    let hit: Vec<bool> = (0..n).map(|i| lin[i] > factor * noise[i]).collect();

    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if !hit[i] {
            i += 1;
            continue;
        }
        // Signal so lange verlängern, wie Lücken kurz genug bleiben.
        let first = i;
        let mut last = i;
        let mut j = i + 1;
        while j < n && j <= last + cfg.max_gap + 1 {
            if hit[j] {
                last = j;
            }
            j += 1;
        }
        let peak = (first..=last).max_by(|&a, &b| lin[a].total_cmp(&lin[b])).expect("nicht leer");
        // Schwerpunkt der Leistung über dem Rauschen.
        let (mut w, mut wf) = (0.0, 0.0);
        for k in first..=last {
            let p = (lin[k] - noise[k]).max(0.0);
            w += p;
            wf += p * k as f64;
        }
        let center = if w > 0.0 { wf / w } else { peak as f64 };
        out.push(Detection {
            offset_hz: (center - (n / 2) as f64) * bin_hz,
            bandwidth_hz: (last - first + 1) as f64 * bin_hz,
            peak_db: spectrum_db[peak],
            noise_db: (10.0 * noise[peak].log10()) as f32,
            first_bin: first,
            last_bin: last,
        });
        i = last + 1;
    }
    out
}

/// Ein über mehrere Frames verfolgtes Signal.
#[derive(Clone, Debug)]
pub struct Track {
    pub id: u64,
    pub offset_hz: f64,
    pub bandwidth_hz: f64,
    pub peak_db: f32,
    pub snr_db: f32,
    /// Zeitpunkte in Sekunden Signalzeit.
    pub first_seen: f64,
    pub last_seen: f64,
    pub hits: u32,
    /// Treffer in aufeinanderfolgenden Frames bis jetzt.
    pub streak: u32,
    /// Getrennte Auftritte: jede Lücke zwischen Treffern beginnt einen neuen.
    pub bursts: u32,
    pub confirmed: bool,
    /// Aufsummierte Klassenwahrscheinlichkeiten aller Klassifikationen.
    pub votes: Vec<f32>,
    pub classified: u32,
    pub last_classified: f64,
}

impl Track {
    /// Wahrscheinlichste Klasse und ihr Anteil an allen Stimmen.
    pub fn class(&self) -> Option<(usize, f32)> {
        let total: f32 = self.votes.iter().sum();
        let (i, &v) = self.votes.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
        (total > 0.0).then(|| (i, v / total))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum TrackEvent {
    Confirmed(u64),
    Lost(u64),
}

/// Ordnet Detektionen über die Zeit Signalen zu.
pub struct Tracker {
    tracks: Vec<Track>,
    next_id: u64,
    classes: usize,
    frame_s: f64,
    /// So lange darf ein bestätigtes Signal ausbleiben, bevor es als
    /// verschwunden gilt; lang genug für Sendepausen von Burstsendern.
    pub hold_s: f64,
    /// So lange wartet ein unbestätigtes Signal auf seinen nächsten Treffer.
    pub candidate_hold_s: f64,
    /// Bestätigt nach so vielen Frames am Stück …
    pub confirm_streak: u32,
    /// … oder nach so vielen getrennten Bursts.
    pub confirm_bursts: u32,
}

impl Tracker {
    /// `frame_s` ist der Abstand der Spektren in Sekunden.
    pub fn new(classes: usize, frame_s: f64) -> Self {
        Self {
            tracks: Vec::new(),
            next_id: 1,
            classes,
            frame_s,
            hold_s: 10.0,
            candidate_hold_s: 3.0,
            // Ein Sprung von 50 ms berührt höchstens drei Frames zu 40 ms.
            confirm_streak: 6,
            confirm_bursts: 5,
        }
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// Verarbeitet die Detektionen eines Frames zur Zeit `t`. Liefert pro
    /// Detektion die Nummer des zugeordneten Signals sowie Ereignisse.
    pub fn update(&mut self, t: f64, detections: &[Detection], bin_hz: f64) -> (Vec<u64>, Vec<TrackEvent>) {
        let mut events = Vec::new();
        let mut ids = vec![0; detections.len()];
        let mut taken = vec![false; self.tracks.len()];

        // Starke Detektionen zuerst, damit sie sich die passenden Signale nehmen.
        let mut order: Vec<usize> = (0..detections.len()).collect();
        order.sort_by(|&a, &b| detections[b].peak_db.total_cmp(&detections[a].peak_db));
        for di in order {
            let d = &detections[di];
            let nearest = self
                .tracks
                .iter()
                .enumerate()
                .filter(|(ti, tr)| {
                    // Bestätigte Signale dürfen driften. Unbestätigte müssen genau
                    // treffen, sonst sammelt ein Springer zufällig Treffer.
                    let tolerance = if tr.confirmed {
                        (3.0 * bin_hz).max(0.5 * tr.bandwidth_hz.max(d.bandwidth_hz))
                    } else {
                        (0.6 * bin_hz).max(0.05 * d.bandwidth_hz)
                    };
                    !taken[*ti] && (tr.offset_hz - d.offset_hz).abs() <= tolerance
                })
                .min_by(|a, b| (a.1.offset_hz - d.offset_hz).abs().total_cmp(&(b.1.offset_hz - d.offset_hz).abs()))
                .map(|(ti, _)| ti);
            match nearest {
                Some(ti) => {
                    taken[ti] = true;
                    let tr = &mut self.tracks[ti];
                    // Gleitend mitteln: eine einzelne Messung soll die Frequenz nicht springen lassen.
                    tr.offset_hz = 0.7 * tr.offset_hz + 0.3 * d.offset_hz;
                    tr.bandwidth_hz = 0.7 * tr.bandwidth_hz + 0.3 * d.bandwidth_hz;
                    tr.peak_db = d.peak_db;
                    tr.snr_db = d.snr_db();
                    if t - tr.last_seen <= 1.5 * self.frame_s {
                        tr.streak += 1;
                    } else {
                        tr.streak = 1;
                        tr.bursts += 1;
                    }
                    tr.last_seen = t;
                    tr.hits += 1;
                    if !tr.confirmed && (tr.streak >= self.confirm_streak || tr.bursts >= self.confirm_bursts) {
                        tr.confirmed = true;
                        events.push(TrackEvent::Confirmed(tr.id));
                    }
                    ids[di] = tr.id;
                }
                None => {
                    let id = self.next_id;
                    self.next_id += 1;
                    self.tracks.push(Track {
                        id,
                        offset_hz: d.offset_hz,
                        bandwidth_hz: d.bandwidth_hz,
                        peak_db: d.peak_db,
                        snr_db: d.snr_db(),
                        first_seen: t,
                        last_seen: t,
                        hits: 1,
                        streak: 1,
                        bursts: 1,
                        confirmed: false,
                        votes: vec![0.0; self.classes],
                        classified: 0,
                        last_classified: f64::NEG_INFINITY,
                    });
                    taken.push(true);
                    ids[di] = id;
                }
            }
        }

        let (hold, candidate_hold) = (self.hold_s, self.candidate_hold_s);
        self.tracks.retain(|tr| {
            let alive = t - tr.last_seen <= if tr.confirmed { hold } else { candidate_hold };
            if !alive && tr.confirmed {
                events.push(TrackEvent::Lost(tr.id));
            }
            alive
        });
        (ids, events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{RngExt, SeedableRng};

    /// Rauschboden bei -80 dB mit ±1,5 dB Schwankung, wie beim Spitzendetektor.
    fn floor(n: usize, seed: u64) -> Vec<f32> {
        let mut rng = StdRng::seed_from_u64(seed);
        (0..n).map(|_| -80.0 + rng.random_range(-1.5..1.5)).collect()
    }

    fn add(spec: &mut [f32], center: usize, half_width: usize, level: f32) {
        for v in &mut spec[center - half_width..=center + half_width] {
            *v = v.max(level);
        }
    }

    #[test]
    fn rauschen_allein_loest_nichts_aus() {
        for seed in 0..50 {
            assert!(cfar(&floor(2048, seed), 1000.0, &CfarConfig::default()).is_empty());
        }
    }

    #[test]
    fn findet_signale_mit_frequenz_und_breite() {
        let mut s = floor(2048, 1);
        add(&mut s, 1024 + 300, 0, -40.0); // Träger
        add(&mut s, 1024 - 500, 5, -55.0); // 11 Bins breit
        let d = cfar(&s, 1000.0, &CfarConfig::default());
        assert_eq!(d.len(), 2);
        let wide = d.iter().find(|d| d.offset_hz < 0.0).unwrap();
        assert!((wide.offset_hz + 500_000.0).abs() < 500.0, "{}", wide.offset_hz);
        assert!((wide.bandwidth_hz - 11_000.0).abs() <= 1000.0, "{}", wide.bandwidth_hz);
        let tone = d.iter().find(|d| d.offset_hz > 0.0).unwrap();
        assert!((tone.offset_hz - 300_000.0).abs() < 1.0);
        assert!((tone.snr_db() - 40.0).abs() < 2.0);
    }

    #[test]
    fn starkes_signal_verdeckt_schwachen_nachbarn_nicht() {
        let mut s = floor(2048, 2);
        add(&mut s, 1000, 2, -20.0);
        add(&mut s, 1050, 2, -60.0); // 50 Bins daneben, 40 dB schwächer
        assert_eq!(cfar(&s, 1000.0, &CfarConfig::default()).len(), 2);
    }

    fn det(offset_hz: f64) -> Detection {
        Detection { offset_hz, bandwidth_hz: 5000.0, peak_db: -50.0, noise_db: -80.0, first_bin: 0, last_bin: 0 }
    }

    #[test]
    fn dauersender_wird_bestaetigt_und_geht_verloren() {
        let mut tr = Tracker::new(2, 0.04);
        let mut events = Vec::new();
        for f in 0..6 {
            // Die Frequenz wackelt um einige Hundert Hertz, bleibt aber dasselbe Signal.
            let (ids, ev) = tr.update(f as f64 * 0.04, &[det(10_000.0 + 300.0 * (f % 2) as f64)], 1000.0);
            assert_eq!(ids, vec![1]);
            events.extend(ev);
        }
        assert_eq!(events, vec![TrackEvent::Confirmed(1)]);
        let (_, ev) = tr.update(20.0, &[], 1000.0);
        assert_eq!(ev, vec![TrackEvent::Lost(1)]);
        assert!(tr.tracks().is_empty());
    }

    #[test]
    fn burstsender_wird_nach_fuenf_bursts_bestaetigt() {
        let mut tr = Tracker::new(2, 0.04);
        let mut confirmed_at = None;
        // Alle 1,2 s ein Burst über zwei Frames, wie ein AIS-Kanal.
        for burst in 0..6 {
            for f in 0..2 {
                let t = burst as f64 * 1.2 + f as f64 * 0.04;
                let (_, ev) = tr.update(t, &[det(25_000.0)], 1000.0);
                if ev == vec![TrackEvent::Confirmed(1)] {
                    confirmed_at = Some(burst);
                }
            }
        }
        assert_eq!(confirmed_at, Some(4));
    }

    #[test]
    fn springer_wird_nie_bestaetigt() {
        let mut tr = Tracker::new(2, 0.04);
        // Sprung k liegt auf einer zufälligen Frequenz irgendwo in 850 kHz.
        let hop_freq = |k: u64| StdRng::seed_from_u64(k).random_range(250_000.0..1_100_000.0);
        // 5 Minuten: alle 50 ms ein neuer Sprung, sichtbar in jedem Frame zu
        // 40 ms, den er berührt.
        for f in 0..7500u64 {
            let t = f as f64 * 0.04;
            let (first, last) = ((t / 0.05) as u64, ((t + 0.04) / 0.05).ceil() as u64 - 1);
            let hops: Vec<Detection> = (first..=last).map(|k| det(hop_freq(k))).collect();
            let (_, ev) = tr.update(t, &hops, 1000.0);
            assert!(ev.iter().all(|e| !matches!(e, TrackEvent::Confirmed(_))), "Frame {f}: {ev:?}");
        }
    }

    #[test]
    fn stimmen_ergeben_die_klasse() {
        let mut tr = Tracker::new(3, 0.04);
        tr.update(0.0, &[det(0.0)], 1000.0);
        let t = tr.get_mut(1).unwrap();
        t.votes = vec![0.1, 1.5, 0.4];
        assert_eq!(t.class(), Some((1, 0.75)));
    }
}
