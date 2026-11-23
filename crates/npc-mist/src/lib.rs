//! Optional mistlib P2P integration for tc-town catalog discovery, room
//! presence, and the Avatar Link brain role. All rooms share a persistent
//! node id at `{data_dir}/mist-node-id.txt`.

mod avatar_bridge;
mod avatar_link;
mod catalog;
mod engine;

use std::path::Path;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use npc_core::{Module, ModuleCtx};
use tokio::sync::broadcast::error::RecvError;

use avatar_bridge::{AvatarBridge, HELLO_INTERVAL};
use avatar_link::BrainMessage;
use engine::RoomEvent;

pub fn module(_ctx: &ModuleCtx) -> anyhow::Result<Box<dyn Module>> {
    Ok(Box::new(MistModule))
}

struct MistModule;

#[async_trait]
impl Module for MistModule {
    fn name(&self) -> &'static str {
        "npc-mist"
    }

    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()> {
        if !ctx.config.mist.enabled {
            tracing::debug!("npc-mist: mist.enabled is false, idling until shutdown");
            ctx.shutdown.cancelled().await;
            return Ok(());
        }

        let node_id = load_or_create_node_id(&ctx.data_dir)?;
        let mut avatar = ctx
            .config
            .mist
            .avatar_link
            .enabled
            .then(|| AvatarBridge::new(&ctx, &node_id));
        let mut rooms = vec![catalog::CATALOG_ROOM_ID];
        let custom_room = ctx.config.mist.room_id.trim();
        if !custom_room.is_empty() && !rooms.contains(&custom_room) {
            rooms.push(custom_room);
        }
        if let Some(avatar) = &avatar {
            let room = avatar.room_id.as_str();
            anyhow::ensure!(
                !room.is_empty(),
                "mist.avatar_link.room_id must not be empty"
            );
            if !rooms.contains(&room) {
                rooms.push(room);
            }
        }

        tracing::info!(%node_id, ?rooms, "npc-mist: starting mist engine");
        let mut bus_rx = ctx.bus.subscribe();
        let mut room_rx = engine::start(node_id, &ctx.config.mist.signaling_url).await?;
        for room in rooms {
            engine::join_room(room.to_string());
        }

        let mut failed_rooms = Vec::new();
        let first_tick = tokio::time::Instant::now() + HELLO_INTERVAL;
        let mut heartbeat = tokio::time::interval_at(first_tick, HELLO_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = ctx.shutdown.cancelled() => break,
                _ = heartbeat.tick() => {
                    for room in failed_rooms.drain(..) {
                        engine::join_room(room);
                    }
                    if let Some(avatar) = avatar.as_mut().filter(|avatar| avatar.joined) {
                        avatar.hello();
                    }
                }
                event = room_rx.recv() => {
                    match event {
                        Some(RoomEvent::Joined(room)) => {
                            tracing::info!(%room, "npc-mist: joined room");
                            if let Some(avatar) = avatar.as_mut().filter(|avatar| avatar.room_id == room) {
                                avatar.joined = true;
                                avatar.hello();
                                heartbeat.reset();
                            }
                        }
                        Some(RoomEvent::JoinFailed(room, error)) => {
                            tracing::warn!(%room, %error, "npc-mist: failed to join room, will retry");
                            failed_rooms.push(room);
                        }
                        Some(RoomEvent::Raw(room, data)) => {
                            if let Some(avatar) = avatar.as_mut().filter(|avatar| avatar.joined && avatar.room_id == room) {
                                if avatar.receive(&ctx.bus, &data, Instant::now()) {
                                    avatar.hello();
                                }
                            }
                            if room == catalog::CATALOG_ROOM_ID {
                                let data_dir = ctx.data_dir.clone();
                                let bus = ctx.bus.clone();
                                tokio::spawn(async move {
                                    catalog::handle_raw_event(&data, &data_dir, &bus).await;
                                });
                            }
                        }
                        None => {
                            tracing::warn!("npc-mist: room event channel closed, stopping");
                            break;
                        }
                    }
                }
                event = bus_rx.recv(), if avatar.is_some() => {
                    match event {
                        Ok(event) => {
                            if let Some(avatar) = &mut avatar {
                                if let Some(message) = avatar.from_bus(&event, Instant::now()) {
                                    avatar.send(message);
                                }
                            }
                        }
                        Err(RecvError::Lagged(skipped)) => tracing::warn!(skipped, "npc-mist: bus receiver lagged"),
                        Err(RecvError::Closed) => break,
                    }
                }
            }
        }

        if avatar.is_some_and(|avatar| avatar.joined && avatar.send(BrainMessage::Bye)) {
            // Give the queued broadcast time to reach the send worker before
            // tearing down the sessions.
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        tracing::info!("npc-mist: leaving rooms");
        engine::leave_all_rooms().await
    }
}

/// Loads the persistent mist node id from `{data_dir}/mist-node-id.txt`,
/// generating one (`tc-npc-<8 hex chars>`) on first run.
fn load_or_create_node_id(data_dir: &Path) -> anyhow::Result<String> {
    let path = data_dir.join("mist-node-id.txt");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let node_id = format!("tc-npc-{}", &suffix[..8]);
    std::fs::create_dir_all(data_dir)
        .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", data_dir.display()))?;
    std::fs::write(&path, &node_id)
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", path.display()))?;
    Ok(node_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_persists_across_calls() {
        let dir = std::env::temp_dir().join(format!("npc-mist-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = load_or_create_node_id(&dir).unwrap();
        let second = load_or_create_node_id(&dir).unwrap();
        assert_eq!(first, second);
        assert!(first.starts_with("tc-npc-"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
