use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use uuid::Uuid;

use crate::llm::ChatMessage;

const MAX_HISTORY: usize = 40;

/// In-memory per-session history. Resets on orch restart — good enough for
/// the demo. Bounded at MAX_HISTORY to keep prompt cost in check.
#[derive(Default)]
pub struct SessionStore {
    inner: Mutex<HashMap<String, Vec<ChatMessage>>>,
}

impl SessionStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn new_session_id() -> String {
        format!("sess-{}", Uuid::new_v4())
    }

    pub async fn append(&self, sid: &str, msg: ChatMessage) {
        let mut guard = self.inner.lock().await;
        let entry = guard.entry(sid.to_string()).or_default();
        entry.push(msg);
        let len = entry.len();
        if len > MAX_HISTORY {
            entry.drain(0..(len - MAX_HISTORY));
        }
    }

    pub async fn history(&self, sid: &str) -> Vec<ChatMessage> {
        let guard = self.inner.lock().await;
        guard.get(sid).cloned().unwrap_or_default()
    }
}
