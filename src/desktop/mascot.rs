//! `tc-npc mascot`: the desktop overlay window alone, connecting to an
//! already-running `tc-npc serve`/`app` instance rather than starting one
//! itself.
//!
//! This began life as a standalone binary in a `mascot/` directory with its
//! own Cargo workspace, built to answer one question: can a transparent,
//! undecorated, always-on-top Tauri window composite a three.js/three-vrm
//! WebGL canvas on WebView2, or does the canvas come out as opaque black? It
//! can — measured by pixel-diffing the same screen rect with the window
//! hidden and shown: zero blackened pixels, and text in the window behind it
//! legible straight through. Every window option below is what that
//! measurement was taken against, so treat them as load-bearing rather than
//! stylistic; `background_color`'s alpha in particular (see its comment).
//!
//! That separate binary is gone: shipping two executables to show one
//! character was the wrong seam. The one thing that changed on the way in is
//! `discover_port`, which can now call `npc_core::data_dir()` directly
//! instead of hand-duplicating it across a workspace boundary.

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    window::Color,
    Manager, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_window_state::StateFlags;

use super::PAGE_DEFAULTS_SCRIPT;

/// tc-npc's own default bind port (see `crates/npc-core/src/config.rs`'s
/// `default_server_addr`). Used here only as our fallback, for the case
/// covered by `discover_port`'s doc comment below.
const DEFAULT_PORT: u16 = 47950;

/// File name `npc-server` publishes the bound port to, under
/// `npc_core::data_dir()` (`crates/npc-server/src/lib.rs`'s
/// `SERVER_PORT_FILE_NAME`). Duplicated as a literal because that constant is
/// private to the `npc-server` crate.
const SERVER_PORT_FILE_NAME: &str = "server-port.txt";

/// Reads the port tc-npc actually bound to from
/// `{data_dir}/server-port.txt`, written as plain decimal text. If the file
/// is missing (no server has started yet) or unreadable, falls back to
/// tc-npc's own default port — a stale/wrong port here just means the
/// window shows tc-npc's "can't reach the server" state, not a crash.
fn discover_port() -> u16 {
    let path = npc_core::data_dir().join(SERVER_PORT_FILE_NAME);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|contents| contents.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Show/Hide/Quit -- the bare minimum for a window with no decorations and
/// no taskbar entry to still be controllable. Without at least Quit here,
/// this window would be un-closeable by any normal means.
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &quit])?;

    TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("tc-npc mascot")
        .menu(&menu)
        .on_menu_event(|app, event| {
            let Some(window) = app.get_webview_window("main") else {
                return;
            };
            match event.id.as_ref() {
                "show" => {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                "hide" => {
                    let _ = window.hide();
                }
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .build(app)?;

    Ok(())
}

pub(crate) fn run() -> anyhow::Result<()> {
    tauri::Builder::default()
        // Remembers where the window was left; restricted to size and
        // position on purpose, same as the spike this is ported from --
        // `decorations`/`visible` are fixed by the builder below, not
        // something a saved file should get a vote on.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION)
                .build(),
        )
        .setup(|app| {
            setup_tray(app)?;

            let port = discover_port();
            let url = format!("http://127.0.0.1:{port}/#/avatar");
            eprintln!("tc-npc mascot: loading {url}");

            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title("tc-npc mascot")
                // First-run size and placement only: the window-state plugin
                // overwrites both from the saved file on every later launch.
                .inner_size(360.0, 480.0)
                .decorations(false)
                .transparent(true)
                // `.transparent(true)` alone toggles the window attribute,
                // but on Windows 8+ WebView2 only actually honours
                // transparency when the background colour's alpha channel is
                // exactly 0 -- any other alpha "will be ignored" (see
                // tauri's own doc comment on `background_color`). Set it
                // explicitly rather than hoping the platform default already
                // is (0,0,0,0): this is precisely the "canvas composites as
                // opaque black" failure mode the original spike existed to
                // check for.
                .background_color(Color(0, 0, 0, 0))
                .always_on_top(true)
                .skip_taskbar(true)
                .shadow(false)
                .resizable(true)
                // Runs before the page's own scripts, which is what lets the
                // defaults be read as ordinary stored preferences rather than
                // applied as a visible flash-and-swap.
                .initialization_script(PAGE_DEFAULTS_SCRIPT)
                .build()?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![])
        .run(tauri::generate_context!())?;

    Ok(())
}
