//! Seekarte mit Schiffspositionen: OSM-Kacheln in Web-Mercator, darüber eine
//! SVG-Ebene mit Schiffen und dem Verlauf des ausgewählten Schiffs. Daneben
//! Schiffsliste, Details und Ereignisprotokoll.

use crate::app::AisState;
use crate::format::{age, clock, lat_lon, num};
use crate::geo::{MAX_ZOOM, MIN_ZOOM, TILE, project, unproject, visible_tiles};
use crate::live;
use api::{TrackPoint, Vessel, nav_status_text, ship_type_text};
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::time::Duration;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{KeyboardEvent, PointerEvent, ResizeObserver, WheelEvent};

/// Nach fünf Minuten ohne Meldung wird ein Schiff blass dargestellt.
const STALE_MS: f64 = 5.0 * 60_000.0;
/// Kieler Förde von der Innenstadt bis zum Leuchtturm.
const HOME: (f64, f64) = (54.405, 10.205);

fn category(v: &Vessel) -> &'static str {
    match v.ship_type {
        Some(60..=69) => "passenger",
        Some(70..=79) => "cargo",
        Some(80..=89) => "tanker",
        _ => "other",
    }
}

fn knots(v: Option<f32>) -> Option<String> {
    v.map(|s| format!("{} kn", num(s.into(), 1)))
}

#[component]
pub fn ShipMap(ais: AisState) -> impl IntoView {
    let center = RwSignal::new(HOME);
    let zoom = RwSignal::new(12u8);
    let size = RwSignal::new((0.0, 0.0));
    let selected = RwSignal::new(None::<u32>);
    let track = RwSignal::new(Vec::<TrackPoint>::new());
    let now = RwSignal::new(js_sys::Date::now());
    let dragging = RwSignal::new(false);
    // Letzte Zeigerposition beim Ziehen, und ob seit dem Drücken gezogen wurde.
    let drag = StoredValue::new(None::<(f64, f64)>);
    let moved = StoredValue::new(false);
    let last_wheel = StoredValue::new(0.0);
    let map_ref = NodeRef::<html::Div>::new();

    set_interval(move || now.set(js_sys::Date::now()), Duration::from_secs(1));

    Effect::new(move |_| {
        let Some(el) = map_ref.get() else { return };
        let target = el.clone();
        let on_resize = Closure::<dyn FnMut()>::new(move || {
            size.set((f64::from(target.client_width()), f64::from(target.client_height())));
        });
        ResizeObserver::new(on_resize.as_ref().unchecked_ref()).expect("ResizeObserver").observe(&el);
        on_resize.forget();
    });

    // Weltpixel der linken oberen Ecke.
    let origin = Memo::new(move |_| {
        let (w, h) = size.get();
        let (lat, lon) = center.get();
        let (x, y) = project(lat, lon, zoom.get());
        (x - w / 2.0, y - h / 2.0)
    });
    let tiles = Memo::new(move |_| visible_tiles(origin.get(), size.get(), zoom.get()));
    let screen = move |lat: f64, lon: f64| {
        let (ox, oy) = origin.get();
        let (x, y) = project(lat, lon, zoom.get());
        (x - ox, y - oy)
    };

    let pan = move |dx: f64, dy: f64| {
        let z = zoom.get_untracked();
        let (lat, lon) = center.get_untracked();
        let (x, y) = project(lat, lon, z);
        center.set(unproject(x - dx, y - dy, z));
    };
    // Zoomt so, dass der Punkt unter `at` (Bildschirmpixel) an seiner Stelle bleibt.
    let zoom_at = move |delta: i8, at: Option<(f64, f64)>| {
        let z = zoom.get_untracked();
        let nz = (z as i8 + delta).clamp(MIN_ZOOM as i8, MAX_ZOOM as i8) as u8;
        if nz == z {
            return;
        }
        let (w, h) = size.get_untracked();
        let (mx, my) = at.unwrap_or((w / 2.0, h / 2.0));
        let (ox, oy) = origin.get_untracked();
        let (lat, lon) = unproject(ox + mx, oy + my, z);
        let (px, py) = project(lat, lon, nz);
        zoom.set(nz);
        center.set(unproject(px - mx + w / 2.0, py - my + h / 2.0, nz));
    };
    let local = move |x: i32, y: i32| {
        let r = map_ref.get_untracked().map(|el| el.get_bounding_client_rect());
        r.map_or((0.0, 0.0), |r| (f64::from(x) - r.left(), f64::from(y) - r.top()))
    };

    let select = move |mmsi: u32| {
        selected.set(Some(mmsi));
        track.set(Vec::new());
        spawn_local(async move {
            let url = format!("/api/vessels/{mmsi}/track?limit=300");
            if let Some(t) = live::get_json::<Vec<TrackPoint>>(&url).await
                && selected.get_untracked() == Some(mmsi)
            {
                track.set(t);
            }
        });
    };
    // Neue Positionen des ausgewählten Schiffs an den Verlauf hängen.
    Effect::new(move |_| {
        let Some(m) = selected.get() else { return };
        let p = ais.vessels.with(|vs| {
            let v = vs.iter().find(|v| v.mmsi == m)?;
            Some(TrackPoint { ts_ms: v.last_seen_ms, lat: v.lat?, lon: v.lon? })
        });
        if let Some(p) = p {
            track.update(|t| {
                if t.last().is_none_or(|l| (l.lat, l.lon) != (p.lat, p.lon)) {
                    t.push(p);
                }
            });
        }
    });

    let end_drag = move |_: PointerEvent| {
        drag.set_value(None);
        dragging.set(false);
    };
    let on_key = move |e: KeyboardEvent| {
        let step = 100.0;
        match e.key().as_str() {
            "ArrowLeft" => pan(step, 0.0),
            "ArrowRight" => pan(-step, 0.0),
            "ArrowUp" => pan(0.0, step),
            "ArrowDown" => pan(0.0, -step),
            "+" | "=" => zoom_at(1, None),
            "-" => zoom_at(-1, None),
            _ => return,
        }
        e.prevent_default();
    };

    let ship = move |v: &Vessel| {
        let (x, y) = screen(v.lat?, v.lon?);
        let (w, h) = size.get();
        if x < -60.0 || y < -60.0 || x > w + 60.0 || y > h + 60.0 {
            return None;
        }
        let mmsi = v.mmsi;
        let moving = v.sog.unwrap_or(0.0) > 0.5;
        let class = format!(
            "ship {}{}{}",
            category(v),
            if selected.get() == Some(mmsi) { " selected" } else { "" },
            if now.get() - v.last_seen_ms as f64 > STALE_MS { " stale" } else { "" },
        );
        let label = v.label();
        let symbol = match v.bearing().filter(|_| moving) {
            // Dreieck in Fahrtrichtung, ruhende Schiffe als Kreis.
            Some(b) => view! { <path d="M0,-10 L6,7 L0,3 L-6,7 Z" transform=format!("rotate({b:.0})") /> }.into_any(),
            None => view! { <circle r="5" /> }.into_any(),
        };
        Some(view! {
            <g class=class transform=format!("translate({x:.1} {y:.1})")
                on:click=move |_| if !moved.get_value() { select(mmsi) }>
                <title>{label.clone()}</title>
                {symbol}
                <text x="11" y="4">{label}</text>
            </g>
        })
    };

    let details = move || {
        let m = selected.get()?;
        let v = ais.vessels.with(|vs| vs.iter().find(|v| v.mmsi == m).cloned())?;
        let dash = || "–".to_string();
        let row = |k: &'static str, val: String| view! { <dt>{k}</dt><dd>{val}</dd> };
        Some(view! {
            <section>
                <div class="row">
                    <h2>{v.label()}</h2>
                    <button aria-label="Auswahl aufheben" on:click=move |_| selected.set(None)>"×"</button>
                </div>
                <dl>
                    {row("MMSI", v.mmsi.to_string())}
                    {row("Rufzeichen", v.callsign.clone().filter(|s| !s.is_empty()).unwrap_or_else(dash))}
                    {row("Typ", v.ship_type.map_or("–", ship_type_text).into())}
                    {row("Status", v.nav_status.map_or("–", nav_status_text).into())}
                    {row("Ziel", v.destination.clone().filter(|s| !s.is_empty()).unwrap_or_else(dash))}
                    {row("Abmessungen", v.length_m.zip(v.beam_m).map_or_else(dash, |(l, b)| format!("{l} × {b} m")))}
                    {row("Position", v.lat.zip(v.lon).map_or_else(dash, |(a, b)| lat_lon(a, b)))}
                    {row("Fahrt", knots(v.sog).unwrap_or_else(dash))}
                    {row("Kurs", v.cog.map_or_else(dash, |c| format!("{}°", num(c.into(), 1))))}
                    {row("Steuerkurs", v.heading.map_or_else(dash, |h| format!("{h}°")))}
                    {row("Zuletzt", format!("{} ({})", clock(v.last_seen_ms), age(now.get(), v.last_seen_ms)))}
                    {row("Nachrichten", v.messages.to_string())}
                </dl>
            </section>
        })
    };

    let name_of = move |mmsi: u32| {
        ais.vessels.with(|vs| vs.iter().find(|v| v.mmsi == mmsi).map_or(mmsi.to_string(), Vessel::label))
    };

    view! {
        <div
            class="map"
            class:dragging=move || dragging.get()
            node_ref=map_ref
            tabindex="0"
            aria-label="Seekarte mit Schiffspositionen. Pfeiltasten verschieben, Plus und Minus zoomen."
            on:pointerdown=move |e: PointerEvent| {
                moved.set_value(false);
                if e.button() == 0 {
                    drag.set_value(Some((f64::from(e.client_x()), f64::from(e.client_y()))));
                }
            }
            on:pointermove=move |e: PointerEvent| {
                let Some((lx, ly)) = drag.get_value() else { return };
                let (x, y) = (f64::from(e.client_x()), f64::from(e.client_y()));
                // Kleine Bewegungen zählen noch als Klick.
                if !moved.get_value() && (x - lx).hypot(y - ly) < 4.0 {
                    return;
                }
                moved.set_value(true);
                dragging.set(true);
                drag.set_value(Some((x, y)));
                pan(x - lx, y - ly);
            }
            on:pointerup=end_drag
            on:pointerleave=end_drag
            on:pointercancel=end_drag
            on:wheel=move |e: WheelEvent| {
                // Touchpads liefern viele kleine Ereignisse; eine Stufe pro 150 ms.
                let t = js_sys::Date::now();
                if t - last_wheel.get_value() < 150.0 || e.delta_y() == 0.0 {
                    return;
                }
                last_wheel.set_value(t);
                zoom_at(if e.delta_y() < 0.0 { 1 } else { -1 }, Some(local(e.client_x(), e.client_y())));
            }
            on:keydown=on_key
        >
            <div class="tiles">
                <For
                    each=move || tiles.get()
                    key=|t| *t
                    children=move |t| {
                        view! {
                            <img class="tile" src=t.url() alt="" draggable="false"
                                style:transform=move || {
                                    let (ox, oy) = origin.get();
                                    format!("translate({}px, {}px)", t.x as f64 * TILE - ox, t.y as f64 * TILE - oy)
                                } />
                        }
                    }
                />
            </div>
            <svg class="overlay" width=move || size.get().0 height=move || size.get().1>
                <polyline class="track" points=move || {
                    track.with(|t| t.iter().map(|p| {
                        let (x, y) = screen(p.lat, p.lon);
                        format!("{x:.1},{y:.1}")
                    }).collect::<Vec<_>>().join(" "))
                } />
                {move || ais.vessels.with(|vs| vs.iter().filter_map(ship).collect_view())}
            </svg>
            <div class="zoom">
                <button aria-label="Hineinzoomen" on:click=move |_| zoom_at(1, None)>"+"</button>
                <button aria-label="Herauszoomen" on:click=move |_| zoom_at(-1, None)>"−"</button>
                <button aria-label="Zur Kieler Förde" title="Kieler Förde" on:click=move |_| {
                    center.set(HOME);
                    zoom.set(12);
                }>"⌂"</button>
            </div>
            <div class="attribution">
                "Karte © "<a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener">"OpenStreetMap"</a>"-Mitwirkende"
            </div>
        </div>
        <aside>
            <section>
                <h2>"Schiffe"</h2>
                {move || ais.vessels.with(Vec::is_empty).then(|| view! {
                    <p class="muted">"Noch keine Aussendung empfangen."</p>
                })}
                <ul class="list">
                    {move || ais.vessels.with(|vs| {
                        let mut sorted: Vec<&Vessel> = vs.iter().collect();
                        sorted.sort_by_key(|v| v.label());
                        sorted.into_iter().map(|v| {
                            let mmsi = v.mmsi;
                            let pos = v.lat.zip(v.lon);
                            view! {
                                <li>
                                    <button aria-pressed=move || (selected.get() == Some(mmsi)).to_string()
                                        on:click=move |_| {
                                            select(mmsi);
                                            if let Some(p) = pos { center.set(p) }
                                        }>
                                        <span class=format!("dot {}", category(v))></span>
                                        <span>{v.label()}</span>
                                        <span class="muted">{knots(v.sog).unwrap_or_default()}</span>
                                    </button>
                                </li>
                            }
                        }).collect_view()
                    })}
                </ul>
            </section>
            {details}
            <section>
                <h2>"Ereignisprotokoll"</h2>
                <ul class="log">
                    {move || ais.log.with(|log| log.iter().take(30).map(|m| view! {
                        <li>
                            <div class="row">
                                <span>{format!("{} · Kanal {} · Typ {}", clock(m.ts_ms), m.channel, m.msg_type)}</span>
                                <span class="muted">{name_of(m.mmsi)}</span>
                            </div>
                            {m.nmea.iter().map(|s| view! { <code>{s.clone()}</code> }).collect_view()}
                        </li>
                    }).collect_view())}
                </ul>
            </section>
        </aside>
    }
}
