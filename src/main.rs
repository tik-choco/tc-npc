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

#[derive(Parser)]
#[command(name = "tc-npc", version, about = "Unified AI mascot agent suite")]
struct Cli {
    /// Path to config.json (defaults to ./config.json).
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
    /// Run the agent suite and web server (default when no subcommand is given).
    Run,
    /// Import characters from a tc-town character export JSON file.
    Import { path: PathBuf },
    /// List stored characters.
    Characters,
    /// Print the resolved config path and data directory.
    ConfigPath,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config;

    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run(config_path, cli.no_open).await,
        Command::Import { path } => import(config_path, &path),
        Command::Characters => list_characters_cmd(config_path),
        Command::ConfigPath => config_path_cmd(config_path),
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
}

async fn run(config_path: Option<PathBuf>, no_open: bool) -> anyhow::Result<()> {
    let mut config = Config::load(config_path.as_deref())?;
    if no_open {
        config.server.auto_open = false;
    }
    let config_path = config_path.unwrap_or_else(|| PathBuf::from("config.json"));
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
    if config.tts.enabled || config.stt.enabled {
        spawn_module(&mut handles, &ctx, "npc-speech", npc_speech::module(&ctx));
    }
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

    tracing::info!("tc-npc running — press Ctrl+C to stop");
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown requested, stopping modules...");
    shutdown.cancel();

    for handle in handles {
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
    let characters = npc_core::import_tc_town_export(&data)?;

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
