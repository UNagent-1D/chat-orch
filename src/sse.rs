use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::error::AppError;
use crate::AppState;

const CHANNEL_BUFFER: usize = 32;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct StreamEvent {
    pub kind: String,
    pub text: String,
}

#[derive(Default)]
pub struct SseHub {
    senders: Mutex<HashMap<String, broadcast::Sender<StreamEvent>>>,
}

impl SseHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn sender_for(&self, sid: &str) -> broadcast::Sender<StreamEvent> {
        let mut map = self.senders.lock().expect("sse hub mutex poisoned");
        map.entry(sid.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_BUFFER).0)
            .clone()
    }

    pub fn subscribe(&self, sid: &str) -> broadcast::Receiver<StreamEvent> {
        self.sender_for(sid).subscribe()
    }

    pub fn publish(&self, sid: &str, event: StreamEvent) {
        // Only publish if a sender already exists — if no one subscribed, drop.
        let maybe_tx = {
            let map = self.senders.lock().expect("sse hub mutex poisoned");
            map.get(sid).cloned()
        };
        if let Some(tx) = maybe_tx {
            let _ = tx.send(event);
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    pub session_id: String,
}

pub async fn chat_stream(
    State(state): State<AppState>,
    Query(q): Query<StreamQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    if q.session_id.trim().is_empty() {
        return Err(AppError::BadRequest("session_id is required".into()));
    }

    let rx = state.hub.subscribe(&q.session_id);
    let stream = BroadcastStream::new(rx).filter_map(|res| match res {
        Ok(event) => {
            let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".into());
            Some(Ok(Event::default().data(data)))
        }
        // Lag: drop silently; client can reconnect if it cares.
        Err(_) => None,
    });

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}
