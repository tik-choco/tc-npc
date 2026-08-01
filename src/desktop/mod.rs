//! Desktop (Tauri) integration for `tc-npc app` and `tc-npc mascot`.
//!
//! This whole tree is compiled only for a `--features desktop` build:
//! `mod desktop;` in `main.rs` is itself behind
//! `#[cfg(feature = "desktop")]`, so nothing under here needs its own `cfg`
//! gates — the module simply doesn't exist in a headless build, and Tauri
//! never even appears in that build's dependency graph (see the `desktop`
//! feature in the root `Cargo.toml`).

mod app;
mod mascot;

pub(crate) use app::run as run_app;
pub(crate) use mascot::run as run_mascot;

/// Injected into every desktop webview before the page's own scripts run.
/// Ported verbatim from `mascot/src/main.rs`'s `PAGE_DEFAULTS_SCRIPT` (see
/// its doc comment there for the full rationale): `backdrop=transparent`
/// and `captions=bubble` are the right first-run defaults for a window
/// sitting directly on the desktop, but only as defaults — each write is
/// skipped when the page already set the key itself, so the page's own
/// hover controls still win on every later launch.
pub(crate) const PAGE_DEFAULTS_SCRIPT: &str = r#"
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

// There is deliberately no drag-handle injection script here any more.
//
// While `mascot/` was a separate app loading tc-npc's page as *remote*
// content it had no other option: it couldn't touch the page's source, so it
// injected a 10px transparent `data-tauri-drag-region` strip from Rust. That
// strip worked, but it was invisible and one-third the height of a scrollbar,
// and "grab the blank top edge" is not something anyone discovers — the
// reliable outcome was grabbing the model instead and orbiting the camera.
//
// Now that the window and the page ship in the same binary, the affordance
// belongs in the page, where it can be seen: `AvatarDragHandle` in
// `web/src/views/AvatarView.tsx` renders a 24px strip that fades in a grip
// bar on hover, matching how `.avatar-window-controls` already behaves. It
// carries the same `data-tauri-drag-region` attribute, which Tauri's built-in
// `drag.js` picks up identically — the attribute is simply inert in a plain
// browser or a capture source.
