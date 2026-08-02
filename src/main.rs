//! `tc-npc`: a single Rust binary unifying the former Go agent-talk /
//! agent-memory / agent-speech / agent-vision / agent-action /
//! agent-scheduler services (previously wired together over Redis pub/sub)
//! plus an HTTP/WebSocket server for a web UI, all communicating over an
//! in-process bus. See `docs/ARCHITECTURE.md` for the full picture.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use npc_core::{Bus, Config, Module, ModuleCtx};

// Declared (and everything under it compiled) only for a `--features
// desktop` build. This is the one place `cfg(feature = "desktop")` appears
// for the module *declaration*; `src/desktop/` itself doesn't need to
// sprinkle its own `cfg` attributes since the whole tree simply doesn't
// exist otherwise. The other place it appears is the dispatch wrappers
// (`run_app`/`run_mascot`) right below `main`, which is the whole surface
// this feature gate has in this file.
#[cfg(feature = "desktop")]
mod desktop;

#[derive(Parser)]
#[command(name = "tc-npc", version, about = "Unified AI mascot agent suite")]
struct Cli {
    /// Path to config.json (defaults to `~/.tc-npc/config.json`; an old
    /// `./config.json` from before that default is auto-migrated there on
    /// first run).
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Don't open the web UI in a browser at startup (overrides
    /// `server.auto_open`). Useful under `just watch`, where every rebuild
    /// restarts the binary and would otherwise pop a new browser tab — the
    /// already-open tab reconnects on its own.
    #[arg(long, global = true)]
    no_open: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the desktop app: the full agent suite/server plus a decorated
    /// main window and a transparent desktop-mascot overlay window (default
    /// when no subcommand is given). Requires a `--features desktop` build.
    App,
    /// Desktop mascot window only, connecting to an already-running
    /// `tc-npc serve`/`app` instance. Requires a `--features desktop` build.
    Mascot,
    /// Run the agent suite and web server with no GUI (this was the
    /// previous default subcommand, `run`, kept below as an alias).
    #[command(alias = "run")]
    Serve,
    /// Attach a terminal UI to an already-running instance. A client, not a
    /// server: it speaks the same HTTP/WebSocket API the web UI does, so it
    /// works just as well against a tc-npc on the other end of an SSH
    /// port-forward as against a local one.
    Tui {
        /// Server to attach to. Defaults to whichever port the local server
        /// last actually bound (`~/.tc-npc/server-port.txt`), falling back to
        /// 127.0.0.1:47950 — see `npc_tui::resolve_addr`.
        #[arg(long)]
        addr: Option<std::net::SocketAddr>,
    },
    /// Import characters from a tc-town character export JSON file.
    Import { path: PathBuf },
    /// List stored characters.
    Characters,
    /// Print the resolved config path and data directory.
    ConfigPath,
}

/// No `#[tokio::main]` here: on Windows, Tauri's event loop (`app` and
/// `mascot` below) must own the main thread, so the tokio runtime is built
/// by hand instead and handed to Tauri via `tauri::async_runtime::set`
/// (see `src/desktop/app.rs`). `serve`/`import`/etc. build their own
/// runtime the same way, just to drive a single `block_on` rather than
/// share it with anything.
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config;

    match cli.command.unwrap_or(Command::App) {
        Command::App => run_app(config_path, cli.no_open),
        Command::Mascot => run_mascot(),
        Command::Serve => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(run(config_path, cli.no_open))
        }
        Command::Tui { addr } => {
            // No `init_tracing()` for this one: the TUI owns the terminal
            // (raw mode + alternate screen), and a tracing line written into
            // that from a background task lands in the middle of the drawn
            // frame and corrupts it. What the server is doing shows up in
            // the TUI's own log panel instead, which is the point.
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(npc_tui::run(npc_tui::resolve_addr(addr)?))
        }
        Command::Import { path } => import(config_path, &path),
        Command::Characters => list_characters_cmd(config_path),
        Command::ConfigPath => config_path_cmd(config_path),
    }
}

#[cfg(feature = "desktop")]
fn run_app(config_path: Option<PathBuf>, no_open: bool) -> anyhow::Result<()> {
    desktop::run_app(config_path, no_open)
}

#[cfg(not(feature = "desktop"))]
fn run_app(_config_path: Option<PathBuf>, _no_open: bool) -> anyhow::Result<()> {
    Err(no_desktop_ui_error())
}

#[cfg(feature = "desktop")]
fn run_mascot() -> anyhow::Result<()> {
    desktop::run_mascot()
}

#[cfg(not(feature = "desktop"))]
fn run_mascot() -> anyhow::Result<()> {
    Err(no_desktop_ui_error())
}

#[cfg(not(feature = "desktop"))]
fn no_desktop_ui_error() -> anyhow::Error {
    anyhow::anyhow!(
        "this build has no desktop UI; rebuild with --features desktop, or use `tc-npc serve`"
    )
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
}

/// Result of [`start_modules`]: everything a caller needs to either wait on
/// (headless `serve`) or hand off to something else that owns the shutdown
/// sequencing (`app`, which waits on the Tauri event loop instead of on
/// Ctrl+C — see `src/desktop/app.rs`).
pub(crate) struct StartedModules {
    // Only read by `app` mode (src/desktop/app.rs), to build window URLs and
    // the healthcheck target from `config.server.addr`; `serve`/`run` above
    // never touches it, hence the `allow` for a headless build.
    #[allow(dead_code)]
    pub(crate) config: Arc<Config>,
    pub(crate) shutdown: CancellationToken,
    pub(crate) handles: Vec<JoinHandle<()>>,
}

/// Loads config, wires up the bus, and spawns every enabled module —
/// everything `run` used to do up through starting `npc-server`. Split out
/// so `app` mode (`src/desktop/app.rs`) can reuse exactly this startup path
/// on its own tokio runtime and then hand the resulting windows/event loop
/// to Tauri instead of awaiting Ctrl+C the way `run` does below. Moving this
/// out changes nothing about `run`/`serve`'s own behavior — it calls this
/// and then does exactly what it always did with the result.
pub(crate) async fn start_modules(
    config_path: Option<PathBuf>,
    no_open: bool,
) -> anyhow::Result<StartedModules> {
    let mut config = Config::load(config_path.as_deref())?;
    if no_open {
        config.server.auto_open = false;
    }
    let config_path = Config::resolve_path(config_path.as_deref());
    init_tracing();

    let data_dir = npc_core::data_dir();
    std::fs::create_dir_all(&data_dir)?;

    let config = Arc::new(config);
    let bus = Bus::new();
    let shutdown = CancellationToken::new();
    let ctx = ModuleCtx {
        bus,
        config: config.clone(),
        config_path,
        shutdown: shutdown.clone(),
        data_dir: data_dir.clone(),
    };

    let active = npc_core::active_character(&data_dir, &config)
        .ok()
        .flatten();

    print_banner(&config, active.as_ref());

    let mut handles: Vec<JoinHandle<()>> = Vec::new();

    if config.talk.enabled {
        spawn_module(&mut handles, &ctx, "npc-talk", npc_talk::module(&ctx));
    }
    if config.memory.enabled {
        spawn_module(&mut handles, &ctx, "npc-memory", npc_memory::module(&ctx));
    }
    // Always spawned, like the scheduler and interpreter below: it idles with
    // both audio threads stopped when `tts.enabled` / `stt.enabled` are off,
    // and starts them the moment the web UI turns either on. Gating the spawn
    // on the startup config meant the 音声 toggles did nothing until the app
    // was restarted, which reads as the voice loop being broken.
    spawn_module(&mut handles, &ctx, "npc-speech", npc_speech::module(&ctx));
    if config.vision.enabled {
        spawn_module(&mut handles, &ctx, "npc-vision", npc_vision::module(&ctx));
    }
    if config.action.enabled {
        spawn_module(&mut handles, &ctx, "npc-action", npc_action::module(&ctx));
    }
    // Unlike the other modules, the scheduler is always spawned: it idles
    // when `scheduler.enabled` is false and picks up schedule edits made
    // from the web UI live (via the `npc:config` bus topic), so announcements
    // added at runtime take effect without a restart.
    spawn_module(&mut handles, &ctx, "npc-scheduler", npc_scheduler::module(&ctx));
    // Same deal for the interpreter: it idles when `translation.mode` is
    // `off` and picks up a mode/language change from the web UI live.
    spawn_module(&mut handles, &ctx, "npc-translate", npc_translate::module(&ctx));
    #[cfg(feature = "mist")]
    if config.mist.enabled {
        spawn_module(&mut handles, &ctx, "npc-mist", npc_mist::module(&ctx));
    }
    // The server module is always started: it's how the web UI and mascot
    // client connect, regardless of which other modules are enabled.
    spawn_module(&mut handles, &ctx, "npc-server", npc_server::module(&ctx));

    Ok(StartedModules {
        config,
        shutdown,
        handles,
    })
}

async fn run(config_path: Option<PathBuf>, no_open: bool) -> anyhow::Result<()> {
    let started = start_modules(config_path, no_open).await?;

    tracing::info!("tc-npc running — press Ctrl+C to stop");
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown requested, stopping modules...");
    started.shutdown.cancel();

    for handle in started.handles {
        let _ = handle.await;
    }

    Ok(())
}

fn spawn_module(
    handles: &mut Vec<JoinHandle<()>>,
    ctx: &ModuleCtx,
    name: &'static str,
    constructed: anyhow::Result<Box<dyn Module>>,
) {
    match constructed {
        Ok(module) => {
            let ctx = ctx.clone();
            handles.push(tokio::spawn(async move {
                if let Err(err) = module.run(ctx).await {
                    tracing::error!(module = name, error = %err, "module exited with error");
                }
            }));
        }
        Err(err) => {
            tracing::warn!(module = name, error = %err, "module unavailable");
        }
    }
}

fn print_banner(config: &Config, active: Option<&npc_core::Character>) {
    println!("tc-npc starting");
    println!(
        "  talk={} memory={} tts={} stt={} vision={} action={} scheduler={} mist={}",
        on_off(config.talk.enabled),
        on_off(config.memory.enabled),
        on_off(config.tts.enabled),
        on_off(config.stt.enabled),
        on_off(config.vision.enabled),
        on_off(config.action.enabled),
        on_off(config.scheduler.enabled),
        on_off(config.mist.enabled),
    );
    println!("  translation: {}", config.translation.mode());
    println!("  server: http://{}", config.server.addr);
    match active {
        Some(c) => println!("  character: {} ({})", c.sheet.name, c.id),
        None => println!("  character: (none active — see `tc-npc characters` / `character.active_id`)"),
    }
}

fn on_off(enabled: bool) -> &'static str {
    if enabled {
        "on"
    } else {
        "off"
    }
}

fn import(config_path: Option<PathBuf>, path: &Path) -> anyhow::Result<()> {
    // Loaded only to fail fast on a broken config; import itself doesn't
    // depend on it.
    let _ = Config::load(config_path.as_deref())?;

    let data_dir = npc_core::data_dir();
    std::fs::create_dir_all(&data_dir)?;

    let data = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let characters = npc_core::import_tc_town_export_into(&data, &data_dir)?;

    if characters.is_empty() {
        println!("No characters found in {}", path.display());
        return Ok(());
    }

    for character in &characters {
        npc_core::save_character(&data_dir, character)?;
        println!("imported: {}  ({})", character.sheet.name, character.id);
    }

    println!();
    println!(
        "hint: set `character.active_id` in your config to one of the ids above to make it the active character."
    );

    Ok(())
}

fn list_characters_cmd(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let config = Config::load(config_path.as_deref())?;
    let data_dir = npc_core::data_dir();
    let characters = npc_core::list_characters(&data_dir)?;

    if characters.is_empty() {
        println!("No characters stored. Use `tc-npc import <path>` to import a tc-town export.");
        return Ok(());
    }

    for character in characters {
        let marker = if character.id == config.character.active_id {
            "*"
        } else {
            " "
        };
        println!("{marker} {}  {}", character.id, character.sheet.name);
    }

    Ok(())
}

fn config_path_cmd(config_path: Option<PathBuf>) -> anyhow::Result<()> {
    let resolved = Config::resolve_path(config_path.as_deref());
    println!("config: {}", resolved.display());
    println!("data dir: {}", npc_core::data_dir().display());
    Ok(())
}
