//! Bridge between Avatar Link and the NPC's conversation and playback bus.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use npc_core::{bus::speech_payload, msg, topic, Bus, BusMessage, ModuleCtx};
use uuid::Uuid;

use crate::avatar_link::{self, Audio, BrainMessage, HostMessage};
use crate::engine;

pub const HELLO_INTERVAL: Duration = Duration::from_secs(30);
const HOST_TIMEOUT: Duration = Duration::from_secs(90);
const MOUTH_INTERVAL: Duration = Duration::from_millis(34);
const REQUEST_PREFIX: &str = "avatar-link:";

struct HostPresence {
    last_seen: Instant,
    answered_hello: bool,
}

pub struct AvatarBridge {
    pub room_id: String,
    pub joined: bool,
    node_id: String,
    name: String,
    allow_hosts: Vec<String>,
    hosts: HashMap<String, HostPresence>,
    tts_enabled: bool,
    speaking: bool,
    last_mouth: Option<Instant>,
}

impl AvatarBridge {
    pub fn new(ctx: &ModuleCtx, node_id: &str) -> Self {
        let settings = &ctx.config.mist.avatar_link;
        let name = if settings.name.trim().is_empty() {
            npc_core::active_character(&ctx.data_dir, &ctx.config)
                .ok()
                .flatten()
                .map(|character| character.sheet.name.trim().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "tc-npc".to_string())
        } else {
            settings.name.trim().to_string()
        };

        Self {
            room_id: settings.room_id.trim().to_string(),
            joined: false,
            node_id: node_id.to_string(),
            name,
            allow_hosts: settings.allow_hosts.clone(),
            hosts: HashMap::new(),
            tts_enabled: ctx.config.tts.enabled,
            speaking: false,
            last_mouth: None,
        }
    }

    pub fn hello(&mut self) {
        let now = Instant::now();
        self.hosts
            .retain(|_, host| now.duration_since(host.last_seen) < HOST_TIMEOUT);
        self.send(BrainMessage::Hello {
            name: &self.name,
            caps: avatar_link::CAPS,
        });
    }

    pub fn send(&self, message: BrainMessage<'_>) -> bool {
        let unreliable = matches!(message, BrainMessage::Mouth { .. });
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let result = avatar_link::encode_brain(&self.node_id, Uuid::new_v4(), ts, message)
            .map_err(anyhow::Error::from)
            .and_then(|data| engine::broadcast(&self.room_id, &data, unreliable));
        if let Err(error) = result {
            tracing::debug!(%error, "npc-mist: failed to enqueue Avatar Link message");
            return false;
        }
        true
    }

    /// Updates peer presence and publishes accepted input. Returns whether to
    /// answer this host's hello.
    pub fn receive(&mut self, bus: &Bus, data: &[u8], now: Instant) -> bool {
        let Some(host) = avatar_link::decode_host(data) else {
            return false;
        };
        if host.from == self.node_id
            || (!self.allow_hosts.is_empty() && !self.allow_hosts.contains(&host.from))
        {
            return false;
        }
        if matches!(host.message, HostMessage::Bye) {
            self.hosts.remove(&host.from);
            return false;
        }
        let presence = self.hosts.entry(host.from).or_insert(HostPresence {
            last_seen: now,
            answered_hello: false,
        });
        if now.duration_since(presence.last_seen) >= HOST_TIMEOUT {
            presence.answered_hello = false;
        }
        presence.last_seen = now;

        match host.message {
            HostMessage::Hello { .. } if !presence.answered_hello => {
                presence.answered_hello = true;
                true
            }
            HostMessage::Input { text, speaker, to }
                if to.as_ref().is_none_or(|to| to == &self.node_id) =>
            {
                // Carry the input id through npc-talk's existing request
                // correlation so overlapping turns each keep their own id.
                let request_id = format!("{REQUEST_PREFIX}{}", host.id);
                bus.publish(
                    topic::SENSE,
                    msg::SPEECH,
                    speech_payload(&text, speaker.as_deref(), Some(&request_id), false),
                );
                false
            }
            _ => false,
        }
    }

    pub fn from_bus<'a>(
        &mut self,
        event: &'a BusMessage,
        now: Instant,
    ) -> Option<BrainMessage<'a>> {
        let kind = (event.topic.as_str(), event.env.r#type.as_str());
        let payload = &event.env.payload;
        if kind == (topic::CONFIG, msg::CONFIG_UPDATED) {
            if let Some(enabled) = payload["tts"]["enabled"].as_bool() {
                self.tts_enabled = enabled;
            }
            return None;
        }
        if !self.joined {
            return None;
        }

        let in_reply_to = || payload["request_id"].as_str()?.strip_prefix(REQUEST_PREFIX);
        Some(match kind {
            (topic::CHAT, msg::CHAT_RESPONSE) => BrainMessage::Say {
                line_id: Uuid::new_v4(),
                text: payload["content"].as_str()?,
                audio: if self.tts_enabled {
                    Audio::Brain
                } else {
                    Audio::Host
                },
                in_reply_to: in_reply_to(),
            },
            (topic::CHAT, msg::CHAT_SILENT) => BrainMessage::Silent {
                in_reply_to: in_reply_to(),
                reason: payload["reason"].as_str(),
            },
            (topic::UI, "speaking") => {
                self.speaking = payload["active"].as_bool()?;
                BrainMessage::Speaking {
                    active: self.speaking,
                }
            }
            (topic::UI, "speaking_level") if self.speaking => {
                if self
                    .last_mouth
                    .is_some_and(|last| now.duration_since(last) < MOUTH_INTERVAL)
                {
                    return None;
                }
                let level = payload["level"].as_f64()?.clamp(0.0, 1.0);
                self.last_mouth = Some(now);
                BrainMessage::Mouth { level }
            }
            (topic::INTERRUPT, msg::INTERRUPT | msg::SUSPEND | msg::VOICE_STOP) => {
                BrainMessage::Stop
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use npc_core::Envelope;
    use serde_json::{json, Value};

    fn bridge() -> AvatarBridge {
        AvatarBridge {
            room_id: "room".into(),
            joined: true,
            node_id: "brain-1".into(),
            name: "NPC".into(),
            allow_hosts: vec!["allowed".into()],
            hosts: HashMap::new(),
            tts_enabled: false,
            speaking: false,
            last_mouth: None,
        }
    }

    fn host_message(kind: &str, from: &str, fields: Value) -> Vec<u8> {
        let mut envelope =
            json!({"v": 1, "t": kind, "id": "i1", "from": from, "role": "host", "ts": 1});
        envelope
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        serde_json::to_vec(&envelope).unwrap()
    }

    fn bus_message(topic: &str, kind: &str, payload: Value) -> BusMessage {
        BusMessage {
            topic: topic.into(),
            env: Envelope {
                r#type: kind.into(),
                payload,
            },
        }
    }

    #[test]
    fn accepts_only_allowed_input_for_this_brain_and_keeps_its_id() {
        let mut brain = bridge();
        let bus = Bus::new();
        let mut rx = bus.subscribe();
        let now = Instant::now();
        for (from, to) in [
            ("other", "brain-1"),
            ("allowed", "brain-2"),
            ("allowed", "brain-1"),
        ] {
            let input = host_message(
                "input",
                from,
                json!({"text": "Hi", "speaker": "Viewer", "to": to}),
            );
            brain.receive(&bus, &input, now);
        }
        let input = rx.try_recv().unwrap();
        assert!(rx.try_recv().is_err());
        assert_eq!(input.topic, topic::SENSE);
        assert_eq!(input.env.r#type, msg::SPEECH);
        assert_eq!(
            input.env.payload,
            json!({"content": "Hi", "speaker": "Viewer", "request_id": "avatar-link:i1"})
        );

        let say = bus_message(
            topic::CHAT,
            msg::CHAT_RESPONSE,
            json!({"content": "Hello", "request_id": "avatar-link:i1"}),
        );
        assert!(matches!(
            brain.from_bus(&say, now),
            Some(BrainMessage::Say {
                in_reply_to: Some("i1"),
                audio: Audio::Host,
                ..
            })
        ));

        let update = bus_message(
            topic::CONFIG,
            msg::CONFIG_UPDATED,
            json!({"tts": {"enabled": true}}),
        );
        assert!(brain.from_bus(&update, now).is_none());
        assert!(matches!(
            brain.from_bus(&say, now),
            Some(BrainMessage::Say {
                audio: Audio::Brain,
                ..
            })
        ));
        let unrelated = bus_message(
            topic::CHAT,
            msg::CHAT_RESPONSE,
            json!({"content": "Hello", "request_id": "ws-request"}),
        );
        assert!(matches!(
            brain.from_bus(&unrelated, now),
            Some(BrainMessage::Say {
                in_reply_to: None,
                ..
            })
        ));
        let silent = bus_message(
            topic::CHAT,
            msg::CHAT_SILENT,
            json!({"request_id": "avatar-link:i2", "reason": "declined"}),
        );
        assert!(matches!(
            brain.from_bus(&silent, now),
            Some(BrainMessage::Silent {
                in_reply_to: Some("i2"),
                reason: Some("declined")
            })
        ));
    }

    #[test]
    fn answers_one_hello_per_host_presence() {
        let mut brain = bridge();
        let bus = Bus::new();
        let hello = host_message("hello", "allowed", json!({"name": "Host", "caps": []}));
        let now = Instant::now();
        assert!(brain.receive(&bus, &hello, now));
        assert!(!brain.receive(&bus, &hello, now));
        assert!(brain.receive(&bus, &hello, now + HOST_TIMEOUT));
        let bye = host_message("bye", "allowed", json!({}));
        assert!(!brain.receive(&bus, &bye, now + HOST_TIMEOUT));
        assert!(brain.receive(&bus, &hello, now + HOST_TIMEOUT));
    }

    #[test]
    fn throttles_mouth_while_speaking() {
        let mut brain = bridge();
        let now = Instant::now();
        let level = bus_message(topic::UI, "speaking_level", json!({"level": 1.5}));
        assert!(brain.from_bus(&level, now).is_none());
        let start = bus_message(topic::UI, "speaking", json!({"active": true}));
        assert!(matches!(
            brain.from_bus(&start, now),
            Some(BrainMessage::Speaking { active: true })
        ));
        assert!(matches!(
            brain.from_bus(&level, now),
            Some(BrainMessage::Mouth { level: 1.0 })
        ));
        assert!(brain
            .from_bus(&level, now + Duration::from_millis(33))
            .is_none());
        assert!(matches!(
            brain.from_bus(&level, now + MOUTH_INTERVAL),
            Some(BrainMessage::Mouth { .. })
        ));
        let stop = bus_message(topic::UI, "speaking", json!({"active": false}));
        assert!(matches!(
            brain.from_bus(&stop, now + MOUTH_INTERVAL),
            Some(BrainMessage::Speaking { active: false })
        ));
        assert!(brain.from_bus(&level, now + MOUTH_INTERVAL * 2).is_none());
    }
}
