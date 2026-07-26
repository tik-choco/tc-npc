//! HTTP/WebSocket server and embedded web UI host. See
//! `docs/ARCHITECTURE.md` for the REST/WS protocol this module implements.

mod assets;
mod bus_forward;
mod hub;
mod protocol;
mod rest;
mod ws;

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::routing::{get, post};
use axum::Router;
use npc_core::{Module, ModuleCtx};
use tokio::net::TcpListener;

use hub::{EchoGuard, Hub};

const MAX_BIND_ATTEMPTS: u16 = 20;

#[derive(Clone)]
struct AppState {
    ctx: ModuleCtx,
    hub: Hub,
    echo: Arc<EchoGuard>,
    /// The address actually bound (may differ from `config.server.addr` if
    /// the configured port was taken and we fell back to the next one).
    addr: String,
    /// The latest *saved* config: starts as the startup snapshot
    /// (`ctx.config`) and is replaced on every successful `PUT /api/config`.
    /// `GET /api/config`, masked-secret restoration, and the LLM probe
    /// endpoints all read this — NOT `ctx.config` — so the settings UI
    /// round-trips what was last saved rather than what the process booted
    /// with. (Running modules still hold the startup snapshot; only
    /// scheduler/action map data hot-reload via the `npc:config` topic.)
    current_config: Arc<std::sync::RwLock<Arc<npc_core::Config>>>,
    /// Latest short-term memory summary, cached from `agent:mem` /
    /// `short_term_memory` bus traffic by `bus_forward` so `GET /api/memory`
    /// can answer it without its own bus subscription.
    short_term_memory: Arc<std::sync::Mutex<String>>,
}

impl AppState {
    /// Cheap snapshot of the latest saved config.
    fn current_config(&self) -> Arc<npc_core::Config> {
        self.current_config.read().unwrap().clone()
    }

    fn set_current_config(&self, config: npc_core::Config) {
        *self.current_config.write().unwrap() = Arc::new(config);
    }
}

struct ServerModule;

#[async_trait]
impl Module for ServerModule {
    fn name(&self) -> &'static str {
        "npc-server"
    }

    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()> {
        run_server(ctx).await
    }
}

pub fn module(_ctx: &ModuleCtx) -> anyhow::Result<Box<dyn Module>> {
    Ok(Box::new(ServerModule))
}

async fn run_server(ctx: ModuleCtx) -> anyhow::Result<()> {
    let (listener, bound_addr) = bind_with_retry(&ctx.config.server.addr).await?;
    tracing::info!(addr = %bound_addr, "npc-server: listening");

    let hub = Hub::default();
    let echo = Arc::new(EchoGuard::default());
    let short_term_memory = Arc::new(std::sync::Mutex::new(String::new()));

    bus_forward::spawn(ctx.clone(), hub.clone(), echo.clone(), short_term_memory.clone());

    let state = AppState {
        current_config: Arc::new(std::sync::RwLock::new(ctx.config.clone())),
        ctx: ctx.clone(),
        hub,
        echo,
        addr: bound_addr.to_string(),
        short_term_memory,
    };

    if ctx.config.server.auto_open {
        let url = format!("http://{bound_addr}");
        if let Err(err) = open::that(&url) {
            tracing::warn!(url, error = %err, "npc-server: failed to auto-open browser");
        }
    }

    let app = Router::new()
        .route("/healthz", get(rest::healthz))
        .route("/ws", get(ws::ws_handler))
        .route("/api/state", get(rest::api_state))
        .route(
            "/api/config",
            get(rest::api_get_config).put(rest::api_put_config),
        )
        .route("/api/characters", get(rest::api_list_characters))
        .route("/api/characters/import", post(rest::api_import_characters))
        .route(
            "/api/characters/:id/activate",
            post(rest::api_activate_character),
        )
        .route("/api/scheduler/test", post(rest::api_scheduler_test))
        .route("/api/memory", get(rest::api_memory))
        .route("/api/chat/history", get(rest::api_chat_history))
        .route(
            "/api/people",
            get(rest::api_list_people).post(rest::api_create_person),
        )
        .route(
            "/api/people/:id",
            get(rest::api_get_person)
                .patch(rest::api_update_person)
                .delete(rest::api_delete_person),
        )
        .route("/api/llm/models", post(rest::api_llm_models))
        .route("/api/llm/voices", post(rest::api_llm_voices))
        .fallback(assets::static_handler)
        .with_state(state);

    let shutdown = ctx.shutdown.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await?;

    Ok(())
}

/// Bind to `addr_str`, retrying on the next port up to
/// [`MAX_BIND_ATTEMPTS`] times if the configured port is already in use.
async fn bind_with_retry(addr_str: &str) -> anyhow::Result<(TcpListener, SocketAddr)> {
    let base: SocketAddr = addr_str
        .parse()
        .map_err(|e| anyhow::anyhow!("npc-server: invalid server.addr {addr_str:?}: {e}"))?;

    let mut candidate = base;
    for attempt in 0..MAX_BIND_ATTEMPTS {
        match TcpListener::bind(candidate).await {
            Ok(listener) => {
                if attempt > 0 {
                    tracing::warn!(
                        configured = addr_str,
                        actual = %candidate,
                        "npc-server: configured port was in use, bound to next available port"
                    );
                }
                return Ok((listener, candidate));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                candidate = SocketAddr::new(base.ip(), candidate.port() + 1);
                continue;
            }
            Err(err) => return Err(err.into()),
        }
    }

    anyhow::bail!(
        "npc-server: failed to bind after {MAX_BIND_ATTEMPTS} attempts starting at {addr_str}"
    )
}
