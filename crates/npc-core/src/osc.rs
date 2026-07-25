//! VRChat chatbox OSC output. Fire-and-forget UDP, matching the original Go
//! `notifier.SendVRCChatbox` behavior.
//!
//! Lives in npc-core because more than one module writes to the chatbox:
//! npc-speech (heard speech / spoken replies, `config.vrc.chatbox`) and
//! npc-translate (subtitles, `config.translation.chatbox`).

use std::net::UdpSocket;

use rosc::{OscMessage, OscPacket, OscType};

/// VRChat's chatbox input text limit.
const CHATBOX_MAX_CHARS: usize = 144;

/// Send `text` to the VRChat chatbox at `osc_address` (e.g.
/// `"127.0.0.1:9000"`), truncated to 144 chars. `/chatbox/input` args are
/// `[text, true, false]` (immediately send, no keyboard notification sound).
/// Best-effort: logs and returns on any failure.
pub fn send_chatbox(osc_address: &str, text: &str) {
    let text = truncate_chars(text, CHATBOX_MAX_CHARS);
    let packet = OscPacket::Message(OscMessage {
        addr: "/chatbox/input".to_string(),
        args: vec![OscType::String(text), OscType::Bool(true), OscType::Bool(false)],
    });

    let bytes = match rosc::encoder::encode(&packet) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(error = ?err, "osc: failed to encode vrc chatbox message");
            return;
        }
    };

    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(error = %err, "osc: failed to bind socket");
            return;
        }
    };

    if let Err(err) = socket.send_to(&bytes, osc_address) {
        tracing::warn!(error = %err, target = %osc_address, "osc: failed to send vrc chatbox message");
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_to_char_count_not_byte_count() {
        let s = "あ".repeat(200);
        let t = truncate_chars(&s, CHATBOX_MAX_CHARS);
        assert_eq!(t.chars().count(), CHATBOX_MAX_CHARS);
    }
}
