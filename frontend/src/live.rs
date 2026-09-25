//! Verbindungen zum Server: WebSockets mit automatischem Neuaufbau und
//! einfache JSON-Abfragen.

use futures::StreamExt;
use gloo_net::websocket::{Message, futures::WebSocket};
use gloo_timers::future::TimeoutFuture;
use leptos::task::spawn_local;
use serde_json::from_str;

fn ws_url(path: &str) -> String {
    let loc = web_sys::window().expect("Browserfenster").location();
    let scheme = if loc.protocol().as_deref() == Ok("https:") { "wss" } else { "ws" };
    format!("{scheme}://{}{path}", loc.host().unwrap_or_default())
}

/// Hält eine WebSocket-Verbindung offen. `on_state` meldet `true` mit der
/// ersten Nachricht und `false` nach einem Abbruch; nach 2 s folgt ein neuer Versuch.
pub fn connect(
    path: &'static str,
    mut on_state: impl FnMut(bool) + 'static,
    mut on_msg: impl FnMut(Message) + 'static,
) {
    spawn_local(async move {
        loop {
            if let Ok(ws) = WebSocket::open(&ws_url(path)) {
                // Die Sendehälfte muss leben, sonst schließt die Verbindung.
                let (_sink, mut stream) = ws.split();
                let mut first = true;
                while let Some(Ok(msg)) = stream.next().await {
                    if first {
                        on_state(true);
                        first = false;
                    }
                    on_msg(msg);
                }
            }
            on_state(false);
            TimeoutFuture::new(2_000).await;
        }
    });
}

/// Holt JSON vom Server; Fehler landen in der Browserkonsole.
pub async fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Option<T> {
    let text = gloo_net::http::Request::get(url).send().await.ok()?.text().await.ok()?;
    match from_str(&text) {
        Ok(v) => Some(v),
        Err(e) => {
            web_sys::console::error_1(&format!("{url}: {e}").into());
            None
        }
    }
}
