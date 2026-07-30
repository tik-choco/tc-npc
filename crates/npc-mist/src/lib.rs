//! npc-mist: optional mistlib P2P integration for tc-npc, gated behind the
//! `mist` cargo feature (see root `Cargo.toml`/`src/main.rs`). Depends on
//! mistlib-core/mistlib-native -- MPL-2.0,
//! https://github.com/tik-choco-lab/mistlib -- as git dependencies pinned
//! to a specific tag (currently v0.6.0).
//!
//! When `config.mist.enabled`, [`module`]'s `run` (see [`MistModule`]):
//!
//! 1. Creates/loads a persistent node id at `{data_dir}/mist-node-id.txt`.
//! 2. Initializes the mistlib engine (falling back to mistlib's own default
//!    signaling URL when `config.mist.signaling_url` is empty) and joins a
//!    single room -- `config.mist.room_id` if set (a plain presence/
//!    messaging room; no special handling of its traffic yet), otherwise
//!    the public tc-town character catalog room (see [`catalog`]).
//! 3. Leaves the room and drops the engine cleanly on `ctx.shutdown`.
//!
//! See `engine.rs`'s module doc comment for *why* it's "a single room" --
//! mistlib-native's engine (at the pinned tag) tracks only one joined room
//! per process.

mod catalog;
mod engine;

use std::path::Path;

use async_trait::async_trait;
use npc_core::{Module, ModuleCtx};

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
            // `src/main.rs` already only spawns this module when
            // `config.mist.enabled`, but guard here too so the module stays
            // well-behaved if ever constructed unconditionally in the
            // future -- idle until shutdown rather than doing nothing and
            // exiting (which `spawn_module` would log as a module having
            // "exited").
            tracing::debug!("npc-mist: mist.enabled is false, idling until shutdown");
            ctx.shutdown.cancelled().await;
            return Ok(());
        }

        let node_id = load_or_create_node_id(&ctx.data_dir)?;

        let signaling_url = if ctx.config.mist.signaling_url.trim().is_empty() {
            mistlib_core::config::Config::new_default().signaling_url
        } else {
            ctx.config.mist.signaling_url.clone()
        };

        let custom_room = ctx.config.mist.room_id.trim().to_string();
        let (room_id, catalog_enabled) = if custom_room.is_empty() {
            (catalog::CATALOG_ROOM_ID.to_string(), true)
        } else {
            // Tradeoff documented in `engine.rs`'s module doc comment:
            // mistlib (at the pinned tag) only tracks one joined room per
            // process, so an explicit `mist.room_id` wins over catalog
            // discovery for this run.
            tracing::warn!(
                room_id = %custom_room,
                catalog_room = catalog::CATALOG_ROOM_ID,
                "npc-mist: mist.room_id is set, so this instance will NOT listen to the tc-town \
                 character catalog room this run -- mistlib's engine (at the pinned tag) tracks \
                 only one joined room per process; unset mist.room_id to discover catalog \
                 characters instead"
            );
            (custom_room, false)
        };

        tracing::info!(
            node_id = %node_id,
            room = %room_id,
            catalog_enabled,
            "npc-mist: starting mist engine"
        );

        let mut raw_rx = engine::start(node_id, signaling_url).await?;

        if let Err(err) = engine::join_room(room_id.clone()).await {
            tracing::error!(error = %err, room = %room_id, "npc-mist: failed to join room");
            return Err(err);
        }
        tracing::info!(room = %room_id, "npc-mist: joined room");

        loop {
            tokio::select! {
                _ = ctx.shutdown.cancelled() => {
                    tracing::info!("npc-mist: shutdown requested, leaving room");
                    if let Err(err) = engine::leave_room().await {
                        tracing::warn!(error = %err, "npc-mist: failed to leave room cleanly");
                    }
                    break;
                }
                event = raw_rx.recv() => {
                    match event {
                        Some(data) if catalog_enabled => {
                            let data_dir = ctx.data_dir.clone();
                            let bus = ctx.bus.clone();
                            tokio::spawn(async move {
                                catalog::handle_raw_event(&data, &data_dir, &bus).await;
                            });
                        }
                        Some(_) => {
                            // Presence/messaging room traffic: no handling
                            // defined for tc-npc yet, just receive-and-drop.
                        }
                        None => {
                            tracing::warn!("npc-mist: raw event channel closed, stopping");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
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
