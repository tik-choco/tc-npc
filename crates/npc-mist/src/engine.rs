//! Thin wrapper around mistlib-native's process-wide engine and room sessions.

use std::sync::OnceLock;

use anyhow::Context;
use mistlib::events::{EVENT_RAW, EVENT_ROOM_JOINED, EVENT_ROOM_JOIN_FAILED};
use mistlib_core::config::{Config, NostrSignalingConfig, SignalingConfig, SignalingMode};
use tokio::sync::mpsc;

pub enum RoomEvent {
    Joined(String),
    JoinFailed(String, String),
    Raw(String, Vec<u8>),
}

// The C callback has no captured state and runs on mistlib's dispatch thread.
static EVENT_TX: OnceLock<mpsc::UnboundedSender<RoomEvent>> = OnceLock::new();

/// # Safety
/// mistlib guarantees the pointer/length pairs are valid for this call.
unsafe extern "C" fn dispatch_room_event(
    event_type: u32,
    room_ptr: *const u8,
    room_len: usize,
    _from_ptr: *const u8,
    _from_len: usize,
    data_ptr: *const u8,
    data_len: usize,
) {
    let Some(tx) = EVENT_TX.get() else { return };
    if !matches!(
        event_type,
        EVENT_RAW | EVENT_ROOM_JOINED | EVENT_ROOM_JOIN_FAILED
    ) {
        return;
    }
    let room = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(room_ptr, room_len) })
        .into_owned();
    let event = match event_type {
        EVENT_ROOM_JOINED => RoomEvent::Joined(room),
        EVENT_ROOM_JOIN_FAILED => {
            let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
            RoomEvent::JoinFailed(room, String::from_utf8_lossy(data).into_owned())
        }
        _ => {
            let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) }.to_vec();
            RoomEvent::Raw(room, data)
        }
    };
    let _ = tx.send(event);
}

fn signaling_config(signaling_url: &str) -> Config {
    let mut config = Config::new_default();
    if signaling_url.is_empty() {
        config.signaling = SignalingConfig {
            mode: SignalingMode::Nostr,
            nostr: Some(NostrSignalingConfig {
                relays: Vec::new(),
                discovery_kind: 25049,
                message_kind: 25050,
                ttl_seconds: 600,
                invite_salt: "tik-choco-v1".to_string(),
                invite_code: "tik-choco-public-v1".to_string(),
                ..Default::default()
            }),
        };
    } else {
        config.use_websocket_signaling_url(signaling_url.to_string());
    }
    config
}

/// Initializes the engine once per process, before scheduling any room joins.
pub async fn start(
    node_id: String,
    signaling_url: &str,
) -> anyhow::Result<mpsc::UnboundedReceiver<RoomEvent>> {
    let config = serde_json::to_vec(&signaling_config(signaling_url.trim()))?;
    let (tx, rx) = mpsc::unbounded_channel();
    EVENT_TX
        .set(tx)
        .map_err(|_| anyhow::anyhow!("mist engine already started in this process"))?;

    tokio::task::spawn_blocking(move || {
        anyhow::ensure!(
            mistlib::app::init_with_config(node_id, &config),
            "mistlib rejected the signaling config"
        );
        mistlib::app::register_event_callback_v2(dispatch_room_event);
        Ok(())
    })
    .await
    .context("mist: initializing mistlib engine")??;

    Ok(rx)
}

/// Schedules an additive join. The callback reports when the session is ready.
pub fn join_room(room_id: String) {
    mistlib::app::join_room(room_id);
}

pub fn broadcast(room_id: &str, data: &[u8], unreliable: bool) -> anyhow::Result<()> {
    let delivery = if unreliable {
        mistlib::app::DELIVERY_UNRELIABLE
    } else {
        mistlib::app::DELIVERY_RELIABLE
    };
    mistlib::app::try_send_message_in_room(room_id.to_string(), String::new(), data, delivery)?;
    Ok(())
}

pub async fn leave_all_rooms() -> anyhow::Result<()> {
    // On a plain thread, mistlib waits for active room teardown to finish.
    let (done, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("mist-shutdown".to_string())
        .spawn(move || {
            mistlib::app::leave_all_rooms();
            let _ = done.send(());
        })?;
    rx.await.context("mist: leaving rooms")
}

/// Fetches content from mistlib's storage, shared by all room sessions.
pub async fn storage_get(cid: &str) -> anyhow::Result<Vec<u8>> {
    use mistlib_core::layers::L0Engine;
    mistlib::layers::NativeL0::new()
        .storage_get(cid)
        .await
        .map_err(|err| anyhow::anyhow!("mist: storage_get({cid}) failed: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signaling_matches_web_namespace_and_supports_websocket_override() {
        let config = signaling_config("");
        let wire = serde_json::to_value(config).unwrap();
        let signaling = &wire["signaling"];
        assert_eq!(signaling["mode"], "nostr");
        assert_eq!(signaling["nostr"]["relays"], serde_json::json!([]));
        assert_eq!(signaling["nostr"]["inviteSalt"], "tik-choco-v1");
        assert_eq!(signaling["nostr"]["inviteCode"], "tik-choco-public-v1");
        assert_eq!(signaling["nostr"]["discoveryKind"], 25049);
        assert_eq!(signaling["nostr"]["messageKind"], 25050);
        assert_eq!(signaling["nostr"]["ttlSeconds"], 600);

        let config = signaling_config("wss://example.test/signal");
        assert_eq!(config.signaling_url, "wss://example.test/signal");
        assert_eq!(config.signaling.mode, SignalingMode::WebSocket);
    }
}
