//! Spektrum und Wasserfall auf zwei Canvas-Flächen, mit Marker, Max-Hold und
//! einstellbarem Pegelbereich. Erkannte Signale erscheinen als farbige Bänder
//! mit ihrer Modulation, daneben Signalliste und Ereignisprotokoll.

use crate::app::SignalState;
use crate::format::{clock, dbfs, mhz, num};
use crate::live;
use api::{Signal, SignalEventKind, SpectrumMeta};
use gloo_net::websocket::Message;
use leptos::html;
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::{Clamped, JsCast};
use web_sys::{CanvasRenderingContext2d as Ctx, HtmlCanvasElement, ImageData, KeyboardEvent, ResizeObserver};

// Wie in style.css; Canvas kann keine CSS-Variablen lesen.
const SEA: &str = "#0e2233";
const GRID: &str = "#24445c";
const MUTED: &str = "#7f9ab0";
const TRACE: &str = "#f2b544";

/// Farbe und CSS-Klasse je Modulation, wie `.cls-*` in style.css.
pub fn class_style(class: &str) -> (&'static str, &'static str) {
    match class {
        "Träger" => ("#b9a6e8", "cls-carrier"),
        "AM" => ("#e0796b", "cls-am"),
        "FM" => ("#8fd18b", "cls-fm"),
        "GMSK" => ("#5cc8e0", "cls-gmsk"),
        "QPSK" => ("#e8a3cf", "cls-qpsk"),
        _ => (MUTED, "cls-noise"),
    }
}

/// Anzeigeeinstellungen, die jede Zeichnung braucht.
#[derive(Clone, Copy)]
struct Scale {
    ref_db: f64,
    range: f64,
}

/// Zustand, der an den Canvas-Elementen hängt und nicht reaktiv sein muss.
struct Scope {
    spec: HtmlCanvasElement,
    sctx: Ctx,
    wf: HtmlCanvasElement,
    wctx: Ctx,
    latest: Option<Vec<f32>>,
    hold: Option<Vec<f32>>,
    lut: Vec<u8>,
}

#[component]
pub fn Spectrum(meta: RwSignal<Option<SpectrumMeta>>, live: RwSignal<bool>, sig: SignalState) -> impl IntoView {
    let ref_db = RwSignal::new(-10.0);
    let range = RwSignal::new(70.0);
    let hold_on = RwSignal::new(false);
    let paused = RwSignal::new(false);
    // Markerposition in CSS-Pixeln vom linken Rand, und was er misst.
    let marker = RwSignal::new(None::<f64>);
    let readout = RwSignal::new(None::<String>);
    let width = RwSignal::new(1.0);

    let scope_ref = NodeRef::<html::Div>::new();
    let spec_ref = NodeRef::<html::Canvas>::new();
    let wf_ref = NodeRef::<html::Canvas>::new();
    let scope = StoredValue::new_local(None::<Scope>);
    let queued = StoredValue::new(false);

    let scale = move || Scale { ref_db: ref_db.get_untracked(), range: range.get_untracked() };
    let draw = move || {
        let m = meta.get_untracked();
        let r = sig.signals.with_untracked(|signals| {
            scope.with_value(|s| s.as_ref().and_then(|s| s.draw(m.as_ref(), scale(), marker.get_untracked(), signals)))
        });
        readout.set(r);
    };
    // Höchstens einmal pro Bildschirmaktualisierung zeichnen.
    let schedule = move || {
        if !queued.get_value() {
            queued.set_value(true);
            request_animation_frame(move || {
                queued.set_value(false);
                draw();
            });
        }
    };

    // Canvas-Kontexte holen, sobald die Elemente im DOM sind.
    Effect::new(move |_| {
        let (Some(el), Some(spec), Some(wf)) = (scope_ref.get(), spec_ref.get(), wf_ref.get()) else { return };
        scope.set_value(Some(Scope {
            sctx: context(&spec),
            wctx: context(&wf),
            spec,
            wf,
            latest: None,
            hold: None,
            lut: waterfall_lut(),
        }));
        let on_resize = Closure::<dyn FnMut()>::new(move || {
            scope.update_value(|s| {
                if let Some(s) = s {
                    width.set(f64::from(s.spec.client_width()));
                    fit(&s.spec);
                    if fit(&s.wf) {
                        s.wctx.set_fill_style_str(SEA);
                        s.wctx.fill_rect(0.0, 0.0, s.wf.width().into(), s.wf.height().into());
                    }
                }
            });
            draw();
        });
        let observer = ResizeObserver::new(on_resize.as_ref().unchecked_ref()).expect("ResizeObserver");
        observer.observe(&el);
        on_resize.forget(); // lebt so lange wie die Seite
    });

    // Neu zeichnen, wenn sich Einstellungen, Marker oder Signale ändern.
    Effect::new(move |_| {
        ref_db.track();
        range.track();
        marker.track();
        meta.track();
        sig.signals.track();
        schedule();
    });
    // Marker auf ein Signal setzen, etwa aus der Signalliste.
    let mark = move |hz: f64| {
        if let Some(m) = meta.get_untracked() {
            let f = (hz - (m.center_hz - m.sample_rate / 2.0)) / m.sample_rate;
            marker.set(Some(f * width.get_untracked()));
        }
    };
    Effect::new(move |_| {
        let on = hold_on.get();
        scope.update_value(|s| {
            if let Some(s) = s {
                s.hold = if on { s.latest.clone() } else { None };
            }
        });
        schedule();
    });

    live::connect(
        "/ws",
        move |ok| live.set(ok),
        move |msg| match msg {
            Message::Text(t) => meta.set(serde_json::from_str(&t).ok()),
            Message::Bytes(b) if !paused.get_untracked() => {
                let v: Vec<f32> =
                    b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
                let hold = hold_on.get_untracked();
                scope.update_value(|s| {
                    if let Some(s) = s {
                        s.push(v, hold, scale());
                    }
                });
                schedule();
            }
            Message::Bytes(_) => {}
        },
    );

    let on_key = move |e: KeyboardEvent| {
        let dir = match e.key().as_str() {
            "ArrowLeft" => -1.0,
            "ArrowRight" => 1.0,
            _ => return,
        };
        e.prevent_default();
        let w = width.get_untracked();
        let step = if e.shift_key() { w / 10.0 } else { 1.0 };
        marker.set(Some((marker.get_untracked().unwrap_or(w / 2.0) + dir * step).clamp(0.0, w - 1.0)));
    };

    view! {
        <div
            class="scope"
            node_ref=scope_ref
            tabindex="0"
            aria-label="Spektrum und Wasserfall. Pfeiltasten bewegen den Marker."
            on:pointermove=move |e| {
                if let Some(el) = scope_ref.get_untracked() {
                    marker.set(Some(f64::from(e.client_x()) - el.get_bounding_client_rect().left()));
                }
            }
            on:pointerleave=move |_| {
                // Mit Tastaturfokus bleibt der Marker stehen.
                let focused = scope_ref.get_untracked().is_some_and(|s| {
                    let el: &web_sys::Element = &s;
                    document().active_element().as_ref() == Some(el)
                });
                if !focused {
                    marker.set(None);
                }
            }
            on:keydown=on_key
        >
            <div class="pane"><canvas node_ref=spec_ref></canvas></div>
            <div class="pane"><canvas node_ref=wf_ref></canvas></div>
            <div class="cursor" hidden=move || readout.with(Option::is_none)
                style:left=move || format!("{}px", marker.get().unwrap_or(0.0))></div>
            <div class="tag" hidden=move || readout.with(Option::is_none)
                style:left=move || format!("{}px", marker.get().unwrap_or(0.0))
                style:transform=move || tag_shift(marker.get().unwrap_or(0.0), width.get())>
                {move || readout.get().unwrap_or_default()}
            </div>
        </div>
        <aside>
            <label>
                <div class="row"><span>"Referenzpegel"</span><output>{move || dbfs(ref_db.get() as f32).replace(",0", "")}</output></div>
                <input type="range" min="-60" max="10" step="1" prop:value=move || ref_db.get()
                    on:input=move |e| ref_db.set(event_target_value(&e).parse().unwrap_or(-10.0)) />
            </label>
            <label>
                <div class="row"><span>"Anzeigebereich"</span><output>{move || format!("{} dB", range.get())}</output></div>
                <input type="range" min="20" max="120" step="5" prop:value=move || range.get()
                    on:input=move |e| range.set(event_target_value(&e).parse().unwrap_or(70.0)) />
            </label>
            <label class="check">
                <input type="checkbox" prop:checked=move || hold_on.get()
                    on:change=move |e| hold_on.set(event_target_checked(&e)) />
                "Max-Hold"
            </label>
            <button on:click=move |_| paused.update(|p| *p = !*p)>
                {move || if paused.get() { "Fortsetzen" } else { "Anhalten" }}
            </button>
            <section>
                <h2>"Signale"</h2>
                {move || sig.signals.with(Vec::is_empty).then(|| view! {
                    <p class="muted">"Noch kein Signal bestätigt."</p>
                })}
                <ul class="list">
                    {move || sig.signals.with(|signals| signals.iter().map(|s| {
                        let hz = s.center_hz;
                        view! {
                            <li>
                                <button title="Marker auf dieses Signal" on:click=move |_| mark(hz)>
                                    <span class=format!("dot {}", class_style(&s.class).1)></span>
                                    <span>{format!("{} · {}", s.class, mhz(s.center_hz))}</span>
                                    <span class="muted">{format!("{} %", (s.confidence * 100.0).round())}</span>
                                </button>
                            </li>
                        }
                    }).collect_view())}
                </ul>
            </section>
            <section>
                <h2>"Ereignisse"</h2>
                <ul class="log">
                    {move || sig.events.with(|events| events.iter().take(15).map(|e| {
                        let what = match e.kind {
                            SignalEventKind::Appeared => "neu",
                            SignalEventKind::Reclassified => "neu eingestuft",
                            SignalEventKind::Lost => "verschwunden",
                        };
                        let s = &e.signal;
                        view! {
                            <li>
                                <div class="row">
                                    <span>{format!("{} · {} {}", clock(e.ts_ms), s.class, what)}</span>
                                    <span class="muted">{mhz(s.center_hz)}</span>
                                </div>
                                <code>{format!(
                                    "{} kHz breit · SNR {} dB · {} % sicher",
                                    num(s.bandwidth_hz / 1e3, 1),
                                    num(s.snr_db.into(), 0),
                                    (s.confidence * 100.0).round()
                                )}</code>
                            </li>
                        }
                    }).collect_view())}
                </ul>
            </section>
            <p class="hint">"Mit der Maus oder den Pfeiltasten misst der Marker Frequenz und Pegel. Umschalt + Pfeil springt weiter."</p>
        </aside>
    }
}

/// Messwert rechts vom Marker, in der rechten Bildhälfte links davon.
fn tag_shift(x: f64, width: f64) -> &'static str {
    if x > width / 2.0 { "translateX(calc(-100% - 8px))" } else { "translateX(8px)" }
}

fn context(c: &HtmlCanvasElement) -> Ctx {
    c.get_context("2d").ok().flatten().expect("2D-Kontext").unchecked_into()
}

fn dpr() -> f64 {
    web_sys::window().map_or(1.0, |w| w.device_pixel_ratio())
}

/// Passt die Pixelgröße an die angezeigte Größe an; `true`, wenn sich etwas geändert hat.
fn fit(c: &HtmlCanvasElement) -> bool {
    let r = c.get_bounding_client_rect();
    let d = dpr();
    let w = ((r.width() * d).round() as u32).max(1);
    let h = ((r.height() * d).round() as u32).max(1);
    if c.width() == w && c.height() == h {
        return false;
    }
    c.set_width(w);
    c.set_height(h);
    true
}

/// Farbskala des Wasserfalls: Meerestiefe bis Signalspitze.
fn waterfall_lut() -> Vec<u8> {
    const STOPS: [(f32, [u8; 3]); 5] = [
        (0.0, [0x0e, 0x22, 0x33]),
        (0.35, [0x1f, 0x5f, 0x7a]),
        (0.6, [0x5c, 0xc8, 0xe0]),
        (0.82, [0xf2, 0xb5, 0x44]),
        (1.0, [0xff, 0xf6, 0xe0]),
    ];
    (0..256)
        .flat_map(|i| {
            let t = i as f32 / 255.0;
            let k = STOPS.iter().position(|s| s.0 >= t).unwrap_or(STOPS.len() - 1).max(1);
            let ((t0, a), (t1, b)) = (STOPS[k - 1], STOPS[k]);
            let f = (t - t0) / (t1 - t0);
            (0..3).map(move |j| (f32::from(a[j]) + (f32::from(b[j]) - f32::from(a[j])) * f) as u8)
        })
        .collect()
}

/// Mehrere Bins pro Pixel: Maximum nehmen, damit schmale Signale nicht verschwinden.
fn to_pixels(v: &[f32], w: usize) -> Vec<f32> {
    let n = v.len();
    (0..w)
        .map(|x| {
            let a = x * n / w;
            let b = ((x + 1) * n / w).clamp(a + 1, n);
            v[a..b].iter().copied().fold(f32::NEG_INFINITY, f32::max)
        })
        .collect()
}

fn freq_at(m: &SpectrumMeta, f: f64) -> f64 {
    m.center_hz - m.sample_rate / 2.0 + f * m.sample_rate
}

impl Scope {
    fn push(&mut self, v: Vec<f32>, hold_on: bool, scale: Scale) {
        if hold_on {
            match &mut self.hold {
                Some(h) if h.len() == v.len() => h.iter_mut().zip(&v).for_each(|(h, v)| *h = h.max(*v)),
                _ => self.hold = Some(v.clone()),
            }
        }
        self.push_waterfall(&v, scale);
        self.latest = Some(v);
    }

    fn push_waterfall(&self, v: &[f32], s: Scale) {
        let (w, h) = (self.wf.width(), self.wf.height());
        let (wf, hf) = (f64::from(w), f64::from(h));
        // Bisheriges Bild eine Zeile nach unten schieben.
        let _ = self.wctx.draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
            &self.wf,
            0.0,
            0.0,
            wf,
            hf - 1.0,
            0.0,
            1.0,
            wf,
            hf - 1.0,
        );
        let lo = s.ref_db - s.range;
        let row: Vec<u8> = to_pixels(v, w as usize)
            .into_iter()
            .flat_map(|p| {
                let k = (((f64::from(p) - lo) / s.range * 255.0) as i32).clamp(0, 255) as usize * 3;
                [self.lut[k], self.lut[k + 1], self.lut[k + 2], 255]
            })
            .collect();
        if let Ok(img) = ImageData::new_with_u8_clamped_array_and_sh(Clamped(&row), w, 1) {
            let _ = self.wctx.put_image_data(&img, 0.0, 0.0);
        }
    }

    /// Zeichnet das Spektrum und liefert den Messwert unter dem Marker.
    fn draw(&self, meta: Option<&SpectrumMeta>, s: Scale, marker: Option<f64>, signals: &[Signal]) -> Option<String> {
        let c = &self.sctx;
        let (w, h, d) = (f64::from(self.spec.width()), f64::from(self.spec.height()), dpr());
        c.set_fill_style_str(SEA);
        c.fill_rect(0.0, 0.0, w, h);
        let meta = meta?;

        // Raster: 10 × 10 Teilungen wie am Messgerät.
        c.set_stroke_style_str(GRID);
        c.set_line_width(1.0);
        c.begin_path();
        for i in 1..10 {
            let x = (f64::from(i) * w / 10.0).round() + 0.5;
            let y = (f64::from(i) * h / 10.0).round() + 0.5;
            c.move_to(x, 0.0);
            c.line_to(x, h);
            c.move_to(0.0, y);
            c.line_to(w, y);
        }
        c.stroke();

        c.set_fill_style_str(MUTED);
        c.set_font(&format!("{}px \"Barlow Semi Condensed\", sans-serif", 13.0 * d));
        c.set_text_baseline("top");
        for i in (0..10).step_by(2) {
            let label = format!("{:.0}", s.ref_db - f64::from(i) * s.range / 10.0).replace('-', "−");
            let _ = c.fill_text(&label, 6.0 * d, f64::from(i) * h / 10.0 + 4.0 * d);
        }
        c.set_text_baseline("bottom");
        for i in (1..10).step_by(2) {
            let t = format!("{:.3}", freq_at(meta, f64::from(i) / 10.0) / 1e6);
            let tw = c.measure_text(&t).map_or(0.0, |m| m.width());
            let _ = c.fill_text(&t, f64::from(i) * w / 10.0 - tw / 2.0, h - 4.0 * d);
        }

        // Erkannte Signale: Band über die belegte Breite, darüber Klasse und Sicherheit.
        // Die Beschriftungen stehen abwechselnd in zwei Zeilen, damit sich
        // benachbarte nicht überdecken.
        let x_of = |hz: f64| (hz - (meta.center_hz - meta.sample_rate / 2.0)) / meta.sample_rate * w;
        c.set_text_baseline("top");
        for (i, sig) in signals.iter().enumerate() {
            let (color, _) = class_style(&sig.class);
            let x0 = x_of(sig.center_hz - sig.bandwidth_hz / 2.0);
            let x1 = x_of(sig.center_hz + sig.bandwidth_hz / 2.0).max(x0 + 3.0 * d);
            c.set_fill_style_str(color);
            c.set_global_alpha(0.18);
            c.fill_rect(x0, 0.0, x1 - x0, h);
            c.set_global_alpha(1.0);
            let label = format!("{} {} %", sig.class, (sig.confidence * 100.0).round());
            let tw = c.measure_text(&label).map_or(0.0, |m| m.width());
            let x = ((x0 + x1) / 2.0 - tw / 2.0).clamp(0.0, w - tw);
            let _ = c.fill_text(&label, x, (24.0 + 16.0 * (i % 2) as f64) * d);
        }

        let trace = |v: &[f32], color: &str, dash: bool| {
            let y = |v: f32| (s.ref_db - f64::from(v)) / s.range * h;
            c.set_stroke_style_str(color);
            c.set_line_width(1.5 * d);
            let pattern = js_sys::Array::new();
            if dash {
                pattern.push(&(4.0 * d).into());
                pattern.push(&(4.0 * d).into());
            }
            let _ = c.set_line_dash(&pattern);
            c.begin_path();
            for (x, p) in to_pixels(v, w as usize).into_iter().enumerate() {
                if x == 0 { c.move_to(0.0, y(p)) } else { c.line_to(x as f64, y(p)) }
            }
            c.stroke();
            let _ = c.set_line_dash(&js_sys::Array::new());
        };
        if let Some(hold) = &self.hold {
            trace(hold, MUTED, true);
        }
        let latest = self.latest.as_ref()?;
        trace(latest, TRACE, false);

        let f = (marker? / f64::from(self.spec.client_width().max(1))).clamp(0.0, 1.0);
        let bin = ((f * latest.len() as f64) as usize).min(latest.len() - 1);
        Some(format!("{}   {}", mhz(freq_at(meta, f)), dbfs(latest[bin])))
    }
}
