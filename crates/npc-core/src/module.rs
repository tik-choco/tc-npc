//! The `Module` trait every subsystem (talk, memory, speech, vision, action,
//! scheduler, server) implements. Each module is constructed by its crate's
//! `module(ctx: &ModuleCtx) -> anyhow::Result<Box<dyn Module>>` factory
//! function and then driven to completion (or cancellation) by `run`.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::bus::Bus;
use crate::config::Config;

/// Shared context handed to every module: the in-process bus, the loaded
/// config, a cancellation token signaling shutdown, and the app's data
/// directory (`~/.tc-npc/`).
#[derive(Clone)]
pub struct ModuleCtx {
    pub bus: Bus,
    pub config: Arc<Config>,
    /// Resolved path of the loaded (or default) config file; modules that
    /// persist settings (e.g. the server's PUT /api/config) write here.
    pub config_path: PathBuf,
    pub shutdown: CancellationToken,
    pub data_dir: PathBuf,
}

/// A long-running subsystem. `run` takes ownership of `self` (boxed) and
/// should return once `ctx.shutdown` is cancelled (or on unrecoverable
/// error).
#[async_trait]
pub trait Module: Send {
    fn name(&self) -> &'static str;

    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()>;
}
