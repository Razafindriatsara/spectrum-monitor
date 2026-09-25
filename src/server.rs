//! HTTP und WebSocket: liefert die Oberfläche aus und streamt Spektren.
//!
//! Protokoll: nach dem Verbindungsaufbau eine Textnachricht mit Metadaten
//! (JSON), danach pro Frame eine Binärnachricht mit `f32`-Werten in dBFS,
//! Little Endian, von der niedrigsten zur höchsten Frequenz.

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

#[derive(Clone)]
pub struct AppState {
    pub frames: broadcast::Sender<Bytes>,
    pub meta_json: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(|| async { Html(include_str!("../web/index.html")) }))
        .route("/ws", get(ws_handler))
        .with_state(state)
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| stream_to_client(socket, state))
}

async fn stream_to_client(mut socket: WebSocket, state: AppState) {
    if socket.send(Message::Text(state.meta_json.clone().into())).await.is_err() {
        return;
    }
    let mut rx = state.frames.subscribe();
    loop {
        match rx.recv().await {
            Ok(frame) => {
                if socket.send(Message::Binary(frame)).await.is_err() {
                    break; // Client hat die Verbindung geschlossen.
                }
            }
            // Langsamer Client: alte Frames überspringen statt zu blockieren.
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        }
    }
}
