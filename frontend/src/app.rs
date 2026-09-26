use crate::format::{mhz, num};
use crate::live;
use crate::map::ShipMap;
use crate::spectrum::Spectrum;
use api::{AisEvent, MessageLog, Signal, SignalEvent, SignalMessage, SpectrumMeta, Vessel};
use gloo_net::websocket::Message;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// So viele Nachrichten hält das Ereignisprotokoll.
const LOG_LEN: usize = 100;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Spectrum,
    Ships,
}

/// Alles, was über AIS bekannt ist. Wird von der App gefüllt, damit Zähler im
/// Kopf und Karte auch dann aktuell sind, wenn die Karte gerade nicht sichtbar ist.
#[derive(Clone, Copy)]
pub struct AisState {
    /// Nach MMSI sortiert.
    pub vessels: RwSignal<Vec<Vessel>>,
    /// Neueste zuerst.
    pub log: RwSignal<Vec<MessageLog>>,
    pub live: RwSignal<bool>,
}

impl AisState {
    fn new() -> Self {
        let s = Self { vessels: RwSignal::new(Vec::new()), log: RwSignal::new(Vec::new()), live: RwSignal::new(false) };
        spawn_local(async move {
            if let Some(list) = live::get_json::<Vec<Vessel>>("/api/vessels").await {
                s.vessels.update(|v| list.into_iter().for_each(|n| upsert(v, n)));
            }
            if let Some(msgs) = live::get_json::<Vec<MessageLog>>(&format!("/api/messages?limit={LOG_LEN}")).await {
                s.log.update(|log| {
                    // Was per WebSocket schon kam, ist neuer; aus dem Abruf nur Älteres anhängen.
                    let oldest = log.last().map_or(i64::MAX, |m| m.ts_ms);
                    log.extend(msgs.into_iter().filter(|m| m.ts_ms < oldest));
                    log.truncate(LOG_LEN);
                });
            }
        });
        live::connect(
            "/ws/ais",
            move |ok| s.live.set(ok),
            move |msg| {
                let Message::Text(text) = msg else { return };
                let Ok(event) = serde_json::from_str::<AisEvent>(&text) else { return };
                s.vessels.update(|v| upsert(v, event.vessel));
                s.log.update(|log| {
                    log.insert(0, event.message);
                    log.truncate(LOG_LEN);
                });
            },
        );
        s
    }
}

/// Erkannte Signale und ihr Ereignisprotokoll aus der Signalüberwachung.
#[derive(Clone, Copy)]
pub struct SignalState {
    /// Nach Frequenz sortiert.
    pub signals: RwSignal<Vec<Signal>>,
    /// Neueste zuerst.
    pub events: RwSignal<Vec<SignalEvent>>,
}

impl SignalState {
    fn new() -> Self {
        let s = Self { signals: RwSignal::new(Vec::new()), events: RwSignal::new(Vec::new()) };
        spawn_local(async move {
            if let Some(list) = live::get_json::<Vec<SignalEvent>>(&format!("/api/signal-events?limit={LOG_LEN}")).await
            {
                s.events.update(|events| {
                    let oldest = events.last().map_or(i64::MAX, |e| e.ts_ms);
                    events.extend(list.into_iter().filter(|e| e.ts_ms < oldest));
                    events.truncate(LOG_LEN);
                });
            }
        });
        live::connect(
            "/ws/signals",
            |_| {},
            move |msg| {
                let Message::Text(text) = msg else { return };
                match serde_json::from_str::<SignalMessage>(&text) {
                    Ok(SignalMessage::Snapshot { signals }) => s.signals.set(signals),
                    Ok(SignalMessage::Event { event }) => s.events.update(|events| {
                        events.insert(0, event);
                        events.truncate(LOG_LEN);
                    }),
                    Err(_) => {}
                }
            },
        );
        s
    }
}

/// Fügt ein Schiff ein oder ersetzt es, wenn der neue Stand nicht älter ist.
fn upsert(list: &mut Vec<Vessel>, v: Vessel) {
    match list.binary_search_by_key(&v.mmsi, |x| x.mmsi) {
        Ok(i) if list[i].last_seen_ms <= v.last_seen_ms => list[i] = v,
        Ok(_) => {}
        Err(i) => list.insert(i, v),
    }
}

#[component]
pub fn App() -> impl IntoView {
    let tab = RwSignal::new(Tab::Spectrum);
    let meta = RwSignal::new(None::<SpectrumMeta>);
    let spectrum_live = RwSignal::new(false);
    let ais = AisState::new();
    let sig = SignalState::new();

    let tab_button = move |t: Tab, label: &'static str| {
        view! {
            <button role="tab" aria-selected=move || (tab.get() == t).to_string() on:click=move |_| tab.set(t)>
                {label}
            </button>
        }
    };
    let status = move |live: RwSignal<bool>| {
        view! {
            <div class="status" class:live=move || live.get() role="status">
                {move || if live.get() { "Live" } else { "Verbinde…" }}
            </div>
        }
    };

    view! {
        <header>
            <h1>"Spectrum Monitor"</h1>
            <nav class="tabs" role="tablist" aria-label="Ansicht">
                {tab_button(Tab::Spectrum, "Spektrum")}
                {tab_button(Tab::Ships, "Schiffe")}
            </nav>
            <Show
                when=move || tab.get() == Tab::Spectrum
                fallback=move || {
                    view! {
                        <div class="readout"><span>"Schiffe"</span><strong>{move || ais.vessels.with(Vec::len)}</strong></div>
                        <div class="readout"><span>"Kanäle"</span><strong>"161,975 / 162,025 MHz"</strong></div>
                        {status(ais.live)}
                    }
                }
            >
                <div class="readout"><span>"Mitte"</span><strong>{move || meta.with(|m| m.as_ref().map_or("–".into(), |m| mhz(m.center_hz)))}</strong></div>
                <div class="readout"><span>"Span"</span><strong>{move || meta.with(|m| m.as_ref().map_or("–".into(), |m| format!("{} MHz", num(m.sample_rate / 1e6, 1))))}</strong></div>
                <div class="readout"><span>"RBW"</span><strong>{move || meta.with(|m| m.as_ref().map_or("–".into(), |m| format!("{} kHz", num(m.rbw_hz / 1e3, 2))))}</strong></div>
                <div class="readout"><span>"Signale"</span><strong>{move || sig.signals.with(Vec::len)}</strong></div>
                <div class="readout"><span>"Detektor"</span><strong>{move || meta.with(|m| match m.as_ref().map(|m| m.detector.as_str()) {
                    Some("Peak") => "Spitze",
                    Some(_) => "Mittelwert",
                    None => "–",
                })}</strong></div>
                {status(spectrum_live)}
            </Show>
        </header>
        <main>
            <div class="view" hidden=move || tab.get() != Tab::Spectrum>
                <Spectrum meta=meta live=spectrum_live sig=sig />
            </div>
            <div class="view ships" hidden=move || tab.get() != Tab::Ships>
                <ShipMap ais=ais />
            </div>
        </main>
    }
}
