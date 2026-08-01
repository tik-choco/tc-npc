//! `tc-npc app`: starts the full agent suite/server (exactly the same
//! startup path `serve` uses — see [`crate::start_modules`]) on a
//! background tokio runtime, then opens two Tauri windows on the main
//! thread: `main` (decorated, the web UI root) and `mascot` (transparent,
//! always-on-top, the `/#/avatar` overlay). The `mascot` window's options are
//! reproduced verbatim from [`super::mascot`] — see that module's header for
//! the transparency rationale and how it was measured.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    window::Color,
    Manager, RunEvent, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_window_state::StateFlags;

use crate::{start_modules, StartedModules};

use super::PAGE_DEFAULTS_SCRIPT;

/// How long to wait for `npc-server` to start answering `/healthz` before
/// giving up and returning an error, rather than pointing a window at a
/// server that may never come up.
const HEALTHCHECK_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn run(config_path: Option<PathBuf>, no_open: bool) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    // Lets Tauri (and its plugins, e.g. tauri-plugin-window-state) spawn
    // onto the same runtime that's driving the agent modules below, instead
    // of spinning up a second tokio runtime of its own.
    tauri::async_runtime::set(rt.handle().clone());

    let StartedModules {
        config,
        shutdown,
        mut handles,
    } = rt.block_on(start_modules(config_path, no_open))?;

    // npc-server binds asynchronously (see crates/npc-server/src/lib.rs's
    // `bind_with_retry`); give it a moment to start answering before
    // pointing windows at it, rather than racing it.
    wait_for_server(&config.server.addr, HEALTHCHECK_TIMEOUT)?;

    let root_url = format!("http://{}/", config.server.addr);
    let avatar_url = format!("http://{}/#/avatar", config.server.addr);

    let app = tauri::Builder::default()
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION)
                .build(),
        )
        .setup(move |app| {
            setup_tray(app)?;

            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(root_url.parse()?))
                .title("tc-npc")
                .inner_size(1024.0, 768.0)
                .build()?;

            // Mirrors src/desktop/mascot.rs's window builder exactly -- every
            // option here was verified against WebView2's transparency
            // behaviour; see that module for why each one is load-bearing.
            WebviewWindowBuilder::new(app, "mascot", WebviewUrl::External(avatar_url.parse()?))
                .title("tc-npc mascot")
                .inner_size(360.0, 480.0)
                .decorations(false)
                .transparent(true)
                .background_color(Color(0, 0, 0, 0))
                .always_on_top(true)
                .skip_taskbar(true)
                .shadow(false)
                .resizable(true)
                .initialization_script(PAGE_DEFAULTS_SCRIPT)
                .build()?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![])
        .build(tauri::generate_context!())?;

    // `App::run` drives the OS event loop on this (the main) thread and,
    // like other winit/tao-based apps, never returns control here on
    // Windows -- the process exits from inside it once the loop stops.
    // `RunEvent::Exit` is the last event delivered before that happens, so
    // it's the only place cleanup can run: cancel the same `shutdown` token
    // `serve` uses and join the module tasks, on the runtime we've kept
    // alive (moved into this closure) for exactly this purpose.
    app.run(move |_app_handle, event| {
        if let RunEvent::Exit = event {
            shutdown.cancel();
            rt.block_on(async {
                for handle in handles.drain(..) {
                    let _ = handle.await;
                }
            });
        }
    });

    Ok(())
}

/// Blocks the calling thread until `addr` answers `GET /healthz` with a 200,
/// or `timeout` elapses. A bare `TcpStream` + hand-written request line
/// rather than an HTTP client crate: this is the only place in `tc-npc` that
/// would need one, and the `desktop` feature already carries enough new
/// dependency weight in Tauri itself.
fn wait_for_server(addr: &str, timeout: Duration) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let deadline = Instant::now() + timeout;
    let mut last_err = None;
    while Instant::now() < deadline {
        match TcpStream::connect(addr) {
            Ok(mut stream) => {
                let request =
                    format!("GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
                if stream.write_all(request.as_bytes()).is_ok() {
                    let mut buf = [0u8; 16];
                    if stream.read(&mut buf).is_ok() && buf.starts_with(b"HTTP/1.1 200") {
                        return Ok(());
                    }
                }
            }
            Err(err) => last_err = Some(err),
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    anyhow::bail!(
        "timed out waiting for tc-npc server at http://{addr} to become ready{}",
        last_err
            .map(|e| format!(" (last connect error: {e})"))
            .unwrap_or_default()
    )
}

/// Tray icon for `app` mode. Show/Hide targets the `mascot` window rather
/// than `main`: `main` already has ordinary decorations and a taskbar entry,
/// so it doesn't need a tray-based show/hide the way the taskbar-less
/// `mascot` window does -- but a toggle for it is included too since it
/// costs nothing to add.
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Mascot", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide Mascot", true, None::<&str>)?;
    let show_main = MenuItem::with_id(app, "show_main", "Show Main Window", true, None::<&str>)?;
    let hide_main = MenuItem::with_id(app, "hide_main", "Hide Main Window", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &show_main, &hide_main, &quit])?;

    TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("tc-npc")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("mascot") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "hide" => {
                if let Some(w) = app.get_webview_window("mascot") {
                    let _ = w.hide();
                }
            }
            "show_main" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "hide_main" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    Ok(())
}
