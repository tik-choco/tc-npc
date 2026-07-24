//! In-process publish/subscribe bus replacing the Redis pub/sub layer used by
//! the original Go agent-* services. The JSON envelope shape (`{"type":
//! ..., "payload": ...}`) and the topic/type string constants are kept
//! identical to the Go originals (see agent-common/pkg/bus) so that ported
//! logic and documentation stay recognizable.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Default capacity of the underlying broadcast channel.
const CHANNEL_CAPACITY: usize = 256;

/// The envelope every module publishes/subscribes with:
/// `{"type": "...", "payload": ...}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub r#type: String,
    pub payload: serde_json::Value,
}

/// A message as delivered to subscribers, tagged with the topic it was
/// published on (subscribers receive every topic and filter as needed).
#[derive(Clone, Debug)]
pub struct BusMessage {
    pub topic: String,
    pub env: Envelope,
}

/// In-process broadcast bus. Cheaply cloneable; all clones share the same
/// underlying channel.
#[derive(Clone)]
pub struct Bus(broadcast::Sender<BusMessage>);

impl Bus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Bus(tx)
    }

    /// Publish `payload` (any `Serialize`) as `msg_type` on `topic`. Errors
    /// serializing the payload are logged and swallowed, matching the
    /// fire-and-forget semantics of the Go bus client's `Publish`.
    pub fn publish(&self, topic: &str, msg_type: &str, payload: impl Serialize) {
        let value = match serde_json::to_value(payload) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(topic, msg_type, error = %err, "bus: failed to serialize payload");
                return;
            }
        };
        let msg = BusMessage {
            topic: topic.to_string(),
            env: Envelope {
                r#type: msg_type.to_string(),
                payload: value,
            },
        };
        // A send error just means there are currently no subscribers; that's
        // fine for a fire-and-forget bus.
        let _ = self.0.send(msg);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<BusMessage> {
        self.0.subscribe()
    }
}

impl Default for Bus {
    fn default() -> Self {
        Self::new()
    }
}

/// Canonical channel names shared across the former agent-* services.
pub mod topic {
    pub const CHAT: &str = "agent:chat";
    pub const MEM: &str = "agent:mem";
    pub const SENSE: &str = "agent:sense";
    pub const INTERRUPT: &str = "agent:interrupt";
    pub const ACTION: &str = "agent:action";
}

/// Canonical envelope `type` values carried inside [`Envelope::r#type`].
pub mod msg {
    pub const CHAT_RESPONSE: &str = "chat_response";
    pub const CHAT_LOG: &str = "chat_log";
    pub const SHORT_TERM_MEMORY: &str = "short_term_memory";
    pub const LONG_TERM_MEMORY: &str = "long_term_memory";
    pub const SPEECH: &str = "speech";
    pub const VISION: &str = "vision";
    pub const ACTION: &str = "action";
    pub const TTS: &str = "tts";
    pub const SUSPEND: &str = "suspend";
    pub const RESUME: &str = "resume";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_subscribe_roundtrip() {
        let bus = Bus::new();
        let mut rx = bus.subscribe();
        bus.publish(topic::CHAT, msg::CHAT_RESPONSE, serde_json::json!({"text": "hi"}));
        let received = rx.recv().await.unwrap();
        assert_eq!(received.topic, topic::CHAT);
        assert_eq!(received.env.r#type, msg::CHAT_RESPONSE);
        assert_eq!(received.env.payload["text"], "hi");
    }
}
