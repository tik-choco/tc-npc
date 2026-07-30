#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! `tc-npc-mascot` -- a SPIKE, not a product. See README.md for the exact
//! question it exists to answer (can a transparent Tauri window composite a
//! three.js/@pixiv/three-vrm WebGL canvas correctly on WebView2, instead of
//! coming out as an opaque black rectangle) and how to read the result.
//!
//! Deliberately tiny: this binary opens one window against tc-npc's already-
//! running `/#/avatar` page and gets out of the way. No chat logic, no
//! state, no config UI -- if this file grows application logic, that logic
//! belongs in tc-npc itself, not in the window shell that displays it.

use std::path::PathBuf;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    window::Color,
    Manager, WebviewUrl, WebviewWindowBuilder,
};

/// tc-npc's own default bind port (see crates/npc-core/src/config.rs,
/// where `"127.0.0.1:47950"` is the hardcoded fallback `addr`). Used here
/// only as OUR fallback, for the case covered by `discover_port`'s doc
/// comment below.
const DEFAULT_PORT: u16 = 47950;

/// Injected into the remote page via `initialization_script` (see `main`)
/// rather than by editing anything under tc-npc's own `web/` tree -- that
/// tree belongs to other work happening in parallel with this spike, and
/// more fundamentally the page is *remote* content served by tc-npc, so this
/// spike's only avenue for adding a drag affordance is Rust-side injection,
/// not editing the page's own source.
///
/// Tauri ships a `data-tauri-drag-region` mechanism out of the box: every
/// window automatically loads a small built-in script (see
/// `tauri-2.11.2/src/window/scripts/drag.js` in the crate source) that
/// listens for `mousedown` on any element carrying that attribute and calls
/// the `start_dragging` command for us. So the only thing missing for a
/// remote page that never asked for this is an element with the attribute
/// in the first place -- which is all this script adds.
///
/// The strip is kept to a 10px sliver along the very top edge rather than
/// covering the whole window: AvatarView.tsx's own hover-revealed controls
/// (`.avatar-window-controls`) sit inset from the top-right corner, and the
/// VRM stage itself is interactive (mouse-drag orbits the camera -- see
/// VrmStage.tsx's `interactive` prop), so claiming the full window as a drag
/// region would fight both of those instead of coexisting with them.
///
/// UNVERIFIED -- see README.md's "dragging" section. This spike was
/// explicitly told not to launch the app to check the visual/transparency
/// result, and the same restriction means this drag mechanism (and the
/// `remote` capability grant in capabilities/main.json it depends on) has
/// never actually been exercised end-to-end. If it turns out not to work,
/// per the spike's own instructions that is an acceptable outcome: the
/// window still opens and sits wherever `inner_size`/the OS places it.
const DRAG_HANDLE_SCRIPT: &str = r#"
(function () {
  function install() {
    if (document.getElementById('__tcnpc_mascot_drag_handle')) return;
    var el = document.createElement('div');
    el.id = '__tcnpc_mascot_drag_handle';
    el.setAttribute('data-tauri-drag-region', '');
    el.style.position = 'fixed';
    el.style.top = '0';
    el.style.left = '0';
    el.style.right = '0';
    el.style.height = '10px';
    // Above the avatar page's own UI (its highest z-index, .avatar-window-
    // controls, is 2) so the strip actually receives the mousedown instead
    // of the canvas swallowing it first.
    el.style.zIndex = '2147483647';
    el.style.background = 'transparent';
    document.documentElement.appendChild(el);
  }
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', install);
  } else {
    install();
  }
})();
"#;

/// Mirrors `npc_core::data_dir()` (crates/npc-core/src/config.rs) exactly --
/// `dirs::home_dir()` + `.tc-npc`. Duplicated in full rather than depending
/// on the npc-core crate: pulling in any of tc-npc's own workspace crates
/// here would reattach this spike's dependency tree to the main workspace,
/// which is the exact coupling `mascot/Cargo.toml`'s own `[workspace]` table
/// exists to avoid.
fn data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tc-npc")
}

/// Reads the port tc-npc actually bound to from `{data_dir}/server-port.txt`,
/// written as plain decimal text. That write is being added concurrently by
/// another worker in this same fan-out, so on a fresh checkout the file may
/// simply not exist yet -- that is the ordinary case, not an error, and is
/// treated identically to "the file exists but contains garbage": fall back
/// to tc-npc's own default port either way. A stale/wrong port here just
/// means the window shows tc-npc's "can't reach the server" state; it can't
/// corrupt anything, so no retry/error UI is worth building for a spike.
fn discover_port() -> u16 {
    let path = data_dir().join("server-port.txt");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|contents| contents.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Show/Hide/Quit -- the bare minimum for a window with no decorations and
/// no taskbar entry to still be controllable. Without at least Quit here,
/// this window would be un-closeable by any normal means, which would be a
/// genuinely hostile thing to leave running on someone's desktop.
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &quit])?;

    TrayIconBuilder::new()
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("tc-npc mascot (spike)")
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

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            setup_tray(app)?;

            let port = discover_port();
            let url = format!("http://127.0.0.1:{port}/#/avatar");
            // Kept as a plain eprintln! rather than pulling in tc-npc's
            // tracing setup -- this binary has no other logging need, and
            // the point of this line is just to make it obvious from a
            // terminal which port a given run actually picked.
            eprintln!("tc-npc-mascot: loading {url}");

            WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.parse()?))
                .title("tc-npc mascot (spike)")
                .inner_size(360.0, 480.0)
                // No `.position(...)` call -- see README.md's "dragging"
                // section. Whatever the OS/WebView2 defaults to is fine for
                // a spike whose only job is to answer the transparency
                // question.
                .decorations(false)
                .transparent(true)
                // `.transparent(true)` alone toggles the window attribute,
                // but tauri-2.11.2's own doc comment on `background_color`
                // (src/webview/webview_window.rs) is explicit that on
                // Windows 8+ WebView2 only actually honours transparency
                // when the background colour's alpha channel is exactly 0
                // -- any other alpha "will be ignored". Set it explicitly
                // rather than hoping the platform default already is (0,0,0,0):
                // this is precisely the "canvas composites as opaque black"
                // failure mode this spike exists to check for, and this is
                // the one concrete mitigation the crate source pointed at.
                .background_color(Color(0, 0, 0, 0))
                .always_on_top(true)
                .skip_taskbar(true)
                .shadow(false)
                .resizable(true)
                .initialization_script(DRAG_HANDLE_SCRIPT)
                .build()?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![])
        .run(tauri::generate_context!())
        .expect("error while running tc-npc-mascot");
}
