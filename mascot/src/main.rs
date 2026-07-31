#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! `tc-npc-mascot` -- puts tc-npc's avatar on the desktop: a transparent,
//! undecorated, always-on-top window showing the NPC's VRM directly over
//! whatever else is on screen.
//!
//! This started as a spike asking whether WebView2 could composite a
//! three.js/@pixiv/three-vrm WebGL canvas in a transparent window at all, or
//! whether it would flatten to an opaque black rectangle. **It can** --
//! measured, not eyeballed; see README.md for the result and how it was
//! checked. So there is no chroma-key or opaque-window fallback here.
//!
//! Deliberately tiny: this binary opens one window against tc-npc's already-
//! running `/#/avatar` page and gets out of the way. No chat logic, no
//! state, no config UI -- if this file grows application logic, that logic
//! belongs in tc-npc itself, not in the window shell that displays it. The
//! two scripts below are the only exception, and both are first-run defaults
//! for the page rather than behaviour of their own.

use std::path::PathBuf;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    window::Color,
    Manager, WebviewUrl, WebviewWindowBuilder,
};
use tauri_plugin_window_state::StateFlags;

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
/// Verified working end-to-end (synthetic press-drag-release on the strip;
/// the window tracked the cursor exactly). That also settles the doubt this
/// carried while it was a spike: the `remote.urls` allowlist in
/// capabilities/main.json (`"http://127.0.0.1:*"`) really does match at
/// runtime, with the port only known at startup -- so the remote page can
/// reach the one IPC command it is granted.
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

/// Gives the avatar page the two defaults a desktop mascot wants, the first
/// time this window ever loads.
///
/// The page's own defaults are the right ones for a browser tab and the
/// wrong ones here:
///
/// - **backdrop**: defaults to the normal opaque app background
///   (`avatar-window` / `var(--bg)`). In a transparent, undecorated window
///   sitting directly on the desktop that comes up opaque and looks exactly
///   like the compositing failure this app was originally built to detect.
/// - **captions**: default to `strip`, the subtitle bar across the bottom.
///   That bar is a full-width plate, so on the desktop it reads as a visible
///   rectangle hanging under the character -- it undoes the transparency it
///   is drawn on top of. `bubble` says the same line as a speech balloon by
///   the model's head, which is what tc-assistant2's mascot did and what
///   suits a character standing on the desktop.
///
/// Two details make writing to the page's own `localStorage` the right move
/// rather than a hack:
///
/// - It is *this window's* storage, not the user's. A Tauri app gets its own
///   WebView2 user-data folder, so the keys set here are invisible to the
///   same page opened in a browser or used as a capture source. A capture
///   setup left on "chroma" with a subtitle strip is not disturbed.
/// - They are first-run defaults, not overrides: each write is skipped when
///   its key already exists, so the page's own hover controls still win and
///   their choices survive the next launch.
const PAGE_DEFAULTS_SCRIPT: &str = r#"
(function () {
  try {
    var defaults = {
      'tc-npc:avatar-window:backdrop': 'transparent',
      'tc-npc:avatar-window:captions': 'bubble'
    };
    for (var key in defaults) {
      if (localStorage.getItem(key) === null) {
        localStorage.setItem(key, defaults[key]);
      }
    }
  } catch (e) {
    // Storage unavailable -- the page falls back to its own defaults, which
    // only costs an opaque window with a subtitle bar.
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

fn main() {
    tauri::Builder::default()
        // Remembers where the window was left, which is the other half of
        // being draggable: without it every launch drops the character back
        // wherever the OS decides, and a mascot that has to be repositioned
        // each time is not one you leave running. Restricted to size and
        // position on purpose -- the default flag set also restores
        // `decorations` and `visible`, and this window's answer to both is
        // fixed by the builder below, not something a saved file should get
        // a vote on.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION)
                .build(),
        )
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
                .title("tc-npc mascot")
                // First-run size and placement only: the window-state plugin
                // overwrites both from the saved file on every later launch.
                .inner_size(360.0, 480.0)
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
                // Both run before the page's own scripts, which is what lets
                // the defaults be read as ordinary stored preferences rather
                // than applied as a visible flash-and-swap.
                .initialization_script(PAGE_DEFAULTS_SCRIPT)
                .initialization_script(DRAG_HANDLE_SCRIPT)
                .build()?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![])
        .run(tauri::generate_context!())
        .expect("error while running tc-npc-mascot");
}
