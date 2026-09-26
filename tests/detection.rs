//! Detektion am vollständigen Simulatorsignal: Spitzendetektor, CFAR, Tracker.
//! Bestätigt werden sollen genau die sechs festen Sender, nicht aber der
//! Frequenzspringer, und keiner darf zwischendurch verloren gehen.

use spectrum_monitor::detect::{CfarConfig, TrackEvent, Tracker, cfar};
use spectrum_monitor::dsp::{Detector, SpectrumEstimator};
use spectrum_monitor::source::{IqSource, SimulatedSource};

#[test]
fn simulator_liefert_genau_die_festen_sender() {
    let mut src = SimulatedSource::new(3);
    let mut est = SpectrumEstimator::new(2048);
    let bin_hz = src.sample_rate() / 2048.0;
    let mut tracker = Tracker::new(1, 0.04);
    let mut buf = vec![Default::default(); 96_000];
    // 12 s Signalzeit.
    for frame in 0..300 {
        src.read(&mut buf);
        let dets = cfar(&est.process(&buf, Detector::Peak), bin_hz, &CfarConfig::default());
        let (_, events) = tracker.update(frame as f64 * 0.04, &dets, bin_hz);
        assert!(!events.iter().any(|e| matches!(e, TrackEvent::Lost(_))), "Frame {frame}: {events:?}");
    }
    let mut found: Vec<f64> =
        tracker.tracks().iter().filter(|t| t.confirmed).map(|t| (t.offset_hz / 1000.0).round()).collect();
    found.sort_by(f64::total_cmp);
    assert_eq!(found, vec![-900.0, -600.0, -450.0, -300.0, -25.0, 25.0]);
}
