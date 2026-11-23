//! JSON payloads for tc-streaming's Avatar Link protocol (`tc-avatar-link/1`).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

const VERSION: u8 = 1;

pub const CAPS: &[&str] = &[
    "hello", "bye", "input", "state", "say", "silent", "speaking", "mouth", "stop",
];

#[derive(Debug, Deserialize)]
pub struct HostEnvelope {
    v: u8,
    role: String,
    #[serde(rename = "ts")]
    _ts: u64,
    pub id: String,
    pub from: String,
    #[serde(flatten)]
    pub message: HostMessage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum HostMessage {
    Hello {
        #[serde(rename = "name")]
        _name: String,
        #[serde(rename = "caps")]
        _caps: Vec<String>,
    },
    Bye,
    Input {
        text: String,
        speaker: Option<String>,
        to: Option<String>,
    },
    State {
        #[serde(rename = "speaking")]
        _speaking: bool,
    },
}

pub fn decode_host(data: &[u8]) -> Option<HostEnvelope> {
    let envelope: HostEnvelope = serde_json::from_slice(data).ok()?;
    (envelope.v == VERSION && envelope.role == "host").then_some(envelope)
}

#[derive(Serialize)]
struct BrainEnvelope<'a> {
    v: u8,
    id: Uuid,
    from: &'a str,
    role: &'static str,
    ts: u64,
    #[serde(flatten)]
    message: BrainMessage<'a>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "t", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum BrainMessage<'a> {
    Hello {
        name: &'a str,
        caps: &'static [&'static str],
    },
    Bye,
    Say {
        line_id: Uuid,
        text: &'a str,
        audio: Audio,
        #[serde(skip_serializing_if = "Option::is_none")]
        in_reply_to: Option<&'a str>,
    },
    Silent {
        #[serde(skip_serializing_if = "Option::is_none")]
        in_reply_to: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<&'a str>,
    },
    Speaking {
        active: bool,
    },
    Mouth {
        level: f64,
    },
    Stop,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Audio {
    Brain,
    Host,
}

pub fn encode_brain(
    from: &str,
    id: Uuid,
    ts: u64,
    message: BrainMessage<'_>,
) -> serde_json::Result<Vec<u8>> {
    serde_json::to_vec(&BrainEnvelope {
        v: VERSION,
        id,
        from,
        role: "brain",
        ts,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_input_with_unknown_fields() {
        let data = br#"{"v":1,"t":"input","id":"i1","from":"h1","role":"host","ts":1,
            "text":"Hi","speaker":"viewer","to":"b1","source":"chat","future":true}"#;
        let envelope = decode_host(data).unwrap();
        assert_eq!(envelope.id, "i1");
        assert_eq!(envelope.from, "h1");
        assert!(matches!(
            envelope.message,
            HostMessage::Input { text, speaker: Some(speaker), to: Some(to) }
                if text == "Hi" && speaker == "viewer" && to == "b1"
        ));
    }

    #[test]
    fn ignores_wrong_version_role_and_unknown_type() {
        for (version, role, kind) in [
            (2, "host", "bye"),
            (1, "brain", "bye"),
            (1, "host", "future"),
        ] {
            let data =
                json!({"v": version, "t": kind, "role": role, "id": "i1", "from": "h1", "ts": 1});
            assert!(decode_host(data.to_string().as_bytes()).is_none());
        }
    }

    #[test]
    fn encodes_say_with_camel_case_fields() {
        let id = Uuid::from_u128(1);
        let line_id = Uuid::from_u128(2);
        let data = encode_brain(
            "b1",
            id,
            123,
            BrainMessage::Say {
                line_id,
                text: "こんにちは",
                audio: Audio::Brain,
                in_reply_to: Some("i1"),
            },
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&data).unwrap(),
            json!({
                "v": 1, "t": "say", "id": id, "from": "b1", "role": "brain", "ts": 123,
                "lineId": line_id, "text": "こんにちは", "audio": "brain", "inReplyTo": "i1"
            })
        );
    }

    #[test]
    fn omits_absent_optional_fields() {
        let data = encode_brain(
            "b1",
            Uuid::nil(),
            123,
            BrainMessage::Silent {
                in_reply_to: None,
                reason: None,
            },
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&data).unwrap(),
            json!({
                "v": 1, "t": "silent", "id": Uuid::nil(), "from": "b1", "role": "brain", "ts": 123
            })
        );
    }
}
