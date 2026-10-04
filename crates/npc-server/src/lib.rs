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
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post, put};
use axum::Router;
use npc_core::{Module, ModuleCtx};
use tokio::net::TcpListener;

use hub::{EchoGuard, Hub};

const MAX_BIND_ATTEMPTS: u16 = 20;

/// File name (under `ctx.data_dir`) that publishes whichever port
/// `bind_with_retry` actually bound, for a client that isn't the process
/// that started this server (e.g. a desktop shell) to discover it. Matches
/// tc-assistant2's own `server-port.txt` naming exactly — same idea, same
/// file name — since a client that already knows how to read one of these
/// should be able to read the other without a second code path.
const SERVER_PORT_FILE_NAME: &str = "server-port.txt";

/// Upload cap for `POST /api/vrm/:file`. VRM models are commonly 10-50 MB;
/// 200 MB leaves room for an unusually heavy one while still bounding how
/// much a single request can buffer in memory.
const MAX_VRM_UPLOAD_BYTES: usize = 200 * 1024 * 1024;

/// Upload cap for `POST /api/sprites/:file`. A sheet is one PNG — the
/// tc-town-derived ones are a grid of 128 px cells, a few hundred KB — so
/// this only needs to clear axum's 2 MB default, not approach the VRM cap.
const MAX_SPRITE_UPLOAD_BYTES: usize = 16 * 1024 * 1024;

/// Upload cap for `POST /api/llm/transcribe/:filename`. Sized for a clip
/// someone would actually drop on the composer — a long uncompressed
/// recording, not a media library.
const MAX_AUDIO_UPLOAD_BYTES: usize = 64 * 1024 * 1024;

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
    /// Rolling buffer of the most recent `affect` snapshots, appended by
    /// `bus_forward` alongside its live WS broadcast so `GET
    /// /api/affect/history` can answer without its own bus subscription —
    /// same pattern as `short_term_memory` above. This is what lets the 感情
    /// tab's trend sparkline seed itself on page load instead of sitting
    /// empty until the NPC speaks again: the browser's own history
    /// (`useNpcSocket`'s `affectHistory`) is filled only by live frames and
    /// starts empty on every reload. Bounded at
    /// `bus_forward::AFFECT_HISTORY_CAP` — see that constant for sizing.
    affect_history: Arc<std::sync::Mutex<std::collections::VecDeque<bus_forward::AffectHistoryEntry>>>,
    /// Whether the cascade voice loop is currently running. npc-speech owns
    /// the real gate; this is the server-side mirror so a newly-connected
    /// client can be told the switch position, and so every open tab sees
    /// the same one. Starts `true`, matching npc-speech's own default.
    voice_active: Arc<std::sync::atomic::AtomicBool>,
    /// Server-side `suspend`/`resume` gate: holds `input`/`event` WS frames
    /// while suspended instead of letting them reach the talk pipeline (the
    /// bus-level `suspend`/`resume` alone only pauses TTS playback, which
    /// doesn't satisfy the extension API contract's "入力・イベントの処理を
    /// 一時停止する"). See `ws::SuspendGate`.
    suspend_gate: Arc<std::sync::Mutex<ws::SuspendGate>>,
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

    // Publish the actually-bound port for out-of-process discovery: the
    // desktop shell (and any other external client) has no other way to
    // learn it when `bind_with_retry` fell back off the configured port.
    // Written as decimal text and nothing else — "just the port number" is
    // the whole contract, matching tc-assistant2's own
    // `~/.tc-assistant/server-port.txt`.
    let port_file = ctx.data_dir.join(SERVER_PORT_FILE_NAME);
    if let Err(err) = std::fs::write(&port_file, bound_addr.port().to_string()) {
        tracing::warn!(
            path = %port_file.display(),
            error = %err,
            "npc-server: failed to write server-port.txt; out-of-process clients won't be able to discover the bound port"
        );
    }

    let hub = Hub::default();
    let echo = Arc::new(EchoGuard::default());
    let short_term_memory = Arc::new(std::sync::Mutex::new(String::new()));
    let affect_history = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));

    bus_forward::spawn(
        ctx.clone(),
        hub.clone(),
        echo.clone(),
        short_term_memory.clone(),
        affect_history.clone(),
    );

    let state = AppState {
        current_config: Arc::new(std::sync::RwLock::new(ctx.config.clone())),
        ctx: ctx.clone(),
        hub,
        echo,
        addr: bound_addr.to_string(),
        short_term_memory,
        affect_history,
        voice_active: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        suspend_gate: Arc::new(std::sync::Mutex::new(ws::SuspendGate::default())),
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
        .route(
            "/api/characters/:id/avatar",
            put(rest::api_set_character_avatar),
        )
        .route("/api/vrm", get(rest::api_list_vrm))
        .route("/api/vrm/file/:file", get(rest::api_get_vrm_file))
        // Before `/api/vrm/:file` so "default" is read as this route, not as
        // a (never-valid) model file name.
        .route("/api/vrm/default", put(rest::api_set_default_avatar))
        .route(
            "/api/vrm/:file",
            // axum's default body limit is 2 MB; VRM models routinely run to
            // tens of megabytes, so the upload route (and only it) gets a
            // limit sized for one. This is a local-only server writing into
            // its own data folder, but the cap still keeps a runaway or
            // mistaken upload from being buffered without bound.
            post(rest::api_upload_vrm)
                .layer(DefaultBodyLimit::max(MAX_VRM_UPLOAD_BYTES))
                .delete(rest::api_delete_vrm),
        )
        .route("/api/sprites", get(rest::api_list_sprites))
        .route("/api/sprites/file/:file", get(rest::api_get_sprite_file))
        .route(
            "/api/sprites/:file",
            // A sheet is a PNG, not a 3D model: it needs headroom over
            // axum's 2 MB default but nowhere near the VRM route's.
            post(rest::api_upload_sprite)
                .layer(DefaultBodyLimit::max(MAX_SPRITE_UPLOAD_BYTES))
                .delete(rest::api_delete_sprite),
        )
        .route("/api/sound", get(rest::api_list_sound))
        .route("/api/sound/reveal", post(rest::api_reveal_sound_folder))
        .route("/api/scheduler/test", post(rest::api_scheduler_test))
        .route("/api/scheduler/export", get(rest::api_scheduler_export))
        .route("/api/scheduler/import", post(rest::api_scheduler_import))
        .route(
            "/api/schedule-profiles",
            get(rest::api_list_schedule_profiles).post(rest::api_save_schedule_profile),
        )
        .route(
            "/api/schedule-profiles/:id/activate",
            post(rest::api_activate_schedule_profile),
        )
        .route(
            "/api/schedule-profiles/:id",
            delete(rest::api_delete_schedule_profile),
        )
        .route("/api/memory", get(rest::api_memory))
        .route("/api/affect/history", get(rest::api_affect_history))
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
        .route("/api/audio/devices", get(rest::api_audio_devices))
        .route("/api/llm/ocr", post(rest::api_llm_ocr))
        .route(
            "/api/llm/transcribe/:filename",
            // An audio clip is the raw body; the 2 MB default would reject a
            // recording of any length. Well under the VRM cap: this is a
            // voice clip, not a model.
            post(rest::api_llm_transcribe).layer(DefaultBodyLimit::max(MAX_AUDIO_UPLOAD_BYTES)),
        )
        .route("/api/llm/models", post(rest::api_llm_models))
        .route("/api/llm/voices", post(rest::api_llm_voices))
        .fallback(assets::static_handler)
        .with_state(state);

    let shutdown = ctx.shutdown.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await?;

    // Best-effort cleanup, not a correctness requirement: `run_server` is the
    // sole writer of `port_file` for this process's entire lifetime, and by
    // this point the listener has fully stopped accepting connections, so
    // there is no concurrent instance this could race against removing this
    // stale entry too early. A hard kill/crash still leaves the file behind
    // with no way to run this cleanup at all — but a stale port number is the
    // smaller problem (a discovery client just fails to connect and can
    // retry/report that plainly) versus building a more elaborate shutdown
    // path solely to close that narrower gap.
    let _ = std::fs::remove_file(&port_file);

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
                    // `info`, not `warn`: falling back to the next port is
                    // the designed behavior for this exact situation (e.g. a
                    // second instance started during development, or a
                    // leftover process from a previous run still holding the
                    // configured port) and the server is running normally
                    // afterward — nothing is actually wrong. `server-port.txt`
                    // (written below, once `bound_addr` is known) is what
                    // makes this safe to not warn about: any out-of-process
                    // client that needs the real port has a way to find it.
                    tracing::info!(
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
