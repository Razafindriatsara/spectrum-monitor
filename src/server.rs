//! HTTP und WebSocket: liefert die Oberfläche aus, streamt Spektren und
//! AIS-Ereignisse und beantwortet Abfragen an die Datenbank. Das Protokoll ist
//! in der Crate `api` beschrieben.

use crate::store::Store;
use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Json};
use axum::routing::get;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

#[derive(Clone)]
pub struct AppState {
    pub frames: broadcast::Sender<Bytes>,
    pub meta_json: Arc<str>,
    pub ais_events: broadcast::Sender<Arc<str>>,
    pub store: Arc<Mutex<Store>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(|| async { Html(include_str!("../web/index.html")) }))
        .route("/ws", get(spectrum_ws))
        .route("/ws/ais", get(ais_ws))
        .route("/api/vessels", get(vessels))
        .route("/api/vessels/{mmsi}/track", get(track))
        .route("/api/messages", get(messages))
        .with_state(state)
}

async fn spectrum_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        if socket.send(Message::Text(state.meta_json.as_ref().into())).await.is_err() {
            return;
        }
        forward(socket, state.frames.subscribe(), Message::Binary).await;
    })
}

async fn ais_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| forward(socket, state.ais_events.subscribe(), |e| Message::Text(e.as_ref().into())))
}

/// Leitet einen Broadcast-Kanal an einen Client weiter, bis er die Verbindung schließt.
async fn forward<T: Clone>(mut socket: WebSocket, mut rx: broadcast::Receiver<T>, to_msg: impl Fn(T) -> Message) {
    loop {
        match rx.recv().await {
            Ok(item) => {
                if socket.send(to_msg(item)).await.is_err() {
                    break;
                }
            }
            // Langsamer Client: Verpasstes überspringen statt zu blockieren.
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        }
    }
}

#[derive(Deserialize)]
struct Limit {
    limit: Option<u32>,
}

/// Führt eine Datenbankabfrage außerhalb der async-Laufzeit aus.
async fn query<T: Send + 'static>(
    store: Arc<Mutex<Store>>,
    f: impl FnOnce(&Store) -> rusqlite::Result<T> + Send + 'static,
) -> Result<Json<T>, (StatusCode, String)> {
    let result = tokio::task::spawn_blocking(move || f(&store.lock().expect("Datenbank-Mutex vergiftet")))
        .await
        .expect("Abfrage abgestürzt");
    result.map(Json).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn vessels(State(state): State<AppState>) -> impl IntoResponse {
    query(state.store, |db| db.vessels()).await
}

async fn track(State(state): State<AppState>, Path(mmsi): Path<u32>, Query(q): Query<Limit>) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(500).min(10_000);
    query(state.store, move |db| db.track(mmsi, limit)).await
}

async fn messages(State(state): State<AppState>, Query(q): Query<Limit>) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(50).min(1_000);
    query(state.store, move |db| db.messages(limit)).await
}
