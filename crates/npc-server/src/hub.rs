//! Client registry for `/ws`: each connected client gets its own `mpsc`
//! channel; [`Hub::broadcast`] fans a frame out to every registered client
//! and [`Hub::send_to`] unicasts to a single one (used for the `hello` frame
//! sent right after a client connects).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::Message;
use tokio::sync::mpsc;

use crate::protocol::ServerMsg;

#[derive(Clone, Default)]
pub struct Hub {
    clients: Arc<Mutex<HashMap<u64, mpsc::UnboundedSender<Message>>>>,
    next_id: Arc<AtomicU64>,
}

impl Hub {
    pub fn register(&self) -> (u64, mpsc::UnboundedReceiver<Message>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        self.clients.lock().unwrap().insert(id, tx);
        (id, rx)
    }

    pub fn unregister(&self, id: u64) {
        self.clients.lock().unwrap().remove(&id);
    }

    pub fn send_to(&self, id: u64, msg: &ServerMsg) {
        let text = match serde_json::to_string(msg) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(error = %err, "npc-server: failed to serialize ws frame");
                return;
            }
        };
        if let Some(tx) = self.clients.lock().unwrap().get(&id) {
            let _ = tx.send(Message::Text(text));
        }
    }

    pub fn broadcast(&self, msg: &ServerMsg) {
        let text = match serde_json::to_string(msg) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(error = %err, "npc-server: failed to serialize ws frame");
                return;
            }
        };
        let clients = self.clients.lock().unwrap();
        for tx in clients.values() {
            let _ = tx.send(Message::Text(text.clone()));
        }
    }
}

/// Tracks text recently sent to the bus as a `speech` sense so the forwarder
/// can skip re-broadcasting the same content as a `sense` frame when it
/// loops back through `agent:sense` (WS `input`/`event` frames already
/// produce a `chat` frame client-side, so echoing it again as `sense` would
/// be a visible duplicate). Entries expire after ~1s.
#[derive(Default)]
pub struct EchoGuard {
    recent: Mutex<VecDeque<(String, Instant)>>,
}

const ECHO_WINDOW: Duration = Duration::from_secs(1);
const ECHO_MAX_ENTRIES: usize = 32;

impl EchoGuard {
    pub fn mark(&self, text: &str) {
        let mut guard = self.recent.lock().unwrap();
        guard.push_back((text.to_string(), Instant::now()));
        while guard.len() > ECHO_MAX_ENTRIES {
            guard.pop_front();
        }
    }

    /// Returns true (and consumes the entry) if `text` was marked within the
    /// last second.
    pub fn was_recent(&self, text: &str) -> bool {
        let mut guard = self.recent.lock().unwrap();
        let now = Instant::now();
        guard.retain(|(_, ts)| now.duration_since(*ts) <= ECHO_WINDOW);
        if let Some(pos) = guard.iter().position(|(t, _)| t == text) {
            guard.remove(pos);
            true
        } else {
            false
        }
    }
}

/// Simple fixed-rate limiter (used to cap `volume` frame forwarding to
/// ~10/s so a chatty source doesn't flood every WS client).
pub struct RateLimiter {
    min_interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub fn per_second(rate: u32) -> Self {
        Self {
            min_interval: Duration::from_millis(1000 / rate.max(1) as u64),
            last: Mutex::new(None),
        }
    }

    pub fn allow(&self) -> bool {
        let mut last = self.last.lock().unwrap();
        let now = Instant::now();
        match *last {
            Some(prev) if now.duration_since(prev) < self.min_interval => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    }
}
