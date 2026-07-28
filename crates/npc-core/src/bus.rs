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
    /// Internal-only (no Go equivalent): carries [`super::msg::CONFIG_UPDATED`]
    /// after `PUT /api/config` persists a new config, so modules that can
    /// hot-reload (scheduler, action map data) pick it up without a restart.
    /// Payload is the complete new `Config` serialized to JSON, unredacted —
    /// this topic must never be forwarded to WS clients.
    pub const CONFIG: &str = "npc:config";
    /// Used by other modules (npc-talk, npc-action, npc-vision, npc-speech,
    /// npc-translate, ...) to push UI-only updates that don't fit the
    /// existing `agent:*` topics (e.g. avatar position, current TTS
    /// playback line, a raw action log line, the affect/drive snapshot).
    /// Consumed only by npc-server's `bus_forward`, which forwards
    /// recognized envelopes on to connected WS clients.
    pub const UI: &str = "npc:ui";
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
    /// Cut the turn that is in flight right now. Published on
    /// [`super::topic::INTERRUPT`] by npc-server when the web UI's 割り込み
    /// button is pressed, and by npc-speech when the mic hears the user
    /// talking over a playing reply (barge-in). Unlike [`SUSPEND`] it carries
    /// no notion of staying off — it is a one-shot "stop what you're doing".
    pub const INTERRUPT: &str = "interrupt";
    /// Turn the cascade voice loop (mic -> VAD/STT -> talk -> TTS) back on.
    /// Published on [`super::topic::INTERRUPT`] by npc-server when the web
    /// UI's voice toggle is switched on. Distinct from [`RESUME`], which only
    /// un-pauses TTS playback and auto-expires after a timeout: the voice
    /// gate is an explicit operator switch that also controls whether the mic
    /// is listened to at all, and it stays where it's put.
    pub const VOICE_START: &str = "voice_start";
    /// Turn the cascade voice loop off — the mic stops feeding the VAD/STT
    /// pipeline and `chat_response` replies stop being spoken. See
    /// [`VOICE_START`].
    pub const VOICE_STOP: &str = "voice_stop";
    /// See [`super::topic::CONFIG`].
    pub const CONFIG_UPDATED: &str = "config_updated";
    /// Published on [`super::topic::UI`] by npc-talk after every
    /// `AffectState::update`, carrying an `AffectSnapshot` (see
    /// `npc_talk::affect`) so the web UI can render the NPC's internal drive
    /// state in real time.
    pub const AFFECT_STATE: &str = "affect_state";
    /// npc-vision が捉えた人物の観察。`topic::SENSE` に publish され、
    /// npc-memory が人物レコードへマージする。
    pub const PERSON_SEEN: &str = "person_seen";
    /// npc-memory が想起した人物プロフィール。`topic::MEM` に publish され、
    /// npc-talk が `{{person_memory}}` として差し込む。
    pub const PERSON_MEMORY: &str = "person_memory";
    /// 人物レコードの新規作成/更新。`topic::UI` に publish され、
    /// npc-server が WS の `person` フレームとして転送する。
    pub const PERSON_UPDATED: &str = "person_updated";
    /// 人物レコードの削除。`topic::UI`。payload は `{"id": "..."}`。
    pub const PERSON_DELETED: &str = "person_deleted";
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
