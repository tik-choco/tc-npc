# tc-npc-mascot (SPIKE)

This is **not** a finished feature. It is the smallest program that can
answer one question:

> Can a transparent, undecorated, always-on-top Tauri 2 window composite a
> three.js/@pixiv/three-vrm WebGL canvas correctly on WebView2 -- or does the
> canvas come out as an opaque black rectangle?

Nothing else lives here. No chat logic, no state, no settings UI. If this
grows beyond "open one window pointed at tc-npc's own avatar page," that
logic belongs somewhere else.

## Why this is a real risk (not a formality)

The sibling project `tc-assistant2` already proves a transparent Tauri
window works for **DOM content** (its mascot is a plain `<div>` with a
`background-image`). Nothing in this monorepo has ever put a **WebGL
canvas** in a transparent window, and that specific combination is known to
be fragile on WebView2: the compositor can flatten the canvas's alpha to
opaque black, or GPU compositing can misbehave in ways plain DOM content
never exercises. That's the failure mode this spike exists to catch.

## What this app does

1. Reads the port tc-npc actually bound to from `~/.tc-npc/server-port.txt`
   (falls back to `47950` if that file doesn't exist yet -- see
   `discover_port()` in `src/main.rs`).
2. Opens one window, built entirely from Rust (`WebviewWindowBuilder` +
   `WebviewUrl::External`), pointed at
   `http://127.0.0.1:<port>/#/avatar` -- tc-npc's own already-served,
   chrome-free avatar route. Nothing is bundled; there is no local frontend
   to bundle, because the whole point is that tc-npc keeps serving this page.
3. The window is: `decorations(false)`, `transparent(true)`,
   `background_color(0,0,0,0)`, `always_on_top(true)`, `skip_taskbar(true)`,
   `shadow(false)`.
4. A tray icon (Show / Hide / Quit) -- see "Closing it" below.
5. A best-effort, **unverified** drag mechanism -- see "Dragging" below.

## How to run it

From `mascot/` (this directory):

```sh
# 1. tc-npc itself must already be running and serving the avatar page --
#    this app has no NPC logic of its own, it only displays tc-npc's page.
#    Run it however you normally do, e.g. from the repo root:
#      cargo run --release
#
# 2. Dev / quick iteration loop (debug build, fast to rebuild):
cd mascot
cargo run

# 3. Release build:
cd mascot
cargo build --release
./target/release/tc-npc-mascot.exe
```

`npx tauri dev` / `npx tauri build` also work from this directory (tauri-cli
2.11.2 is available via npx on this machine) since `tauri.conf.json` sets no
`beforeDevCommand`/`beforeBuildCommand` to proxy through -- but plain `cargo
run`/`cargo build` is simpler here, since there is no bundled frontend for
the CLI to manage. `mascot/` is its own Cargo workspace (see the comment atop
`Cargo.toml`), so none of this touches or is touched by the root `tc-npc`
workspace build.

**This spike deliberately does not launch the app itself to check the
result** -- there is no screenshot tool available in this environment, and
"does the canvas render transparent" can only be judged by a human looking
at a real screen with real desktop content behind the window. That check is
yours to do.

## What to look for (the actual test)

1. Start tc-npc so it's serving `/#/avatar`.
2. Run the mascot app (above). A frameless, always-on-top window should
   appear (default size 360x480, unpositioned -- see "Dragging").
3. **Hover the mouse over the window** to reveal `AvatarView.tsx`'s own
   controls (top-right). Click the background-toggle button (image icon)
   until its label reads **transparent** -- it cycles
   normal -> chroma -> transparent and the choice persists in
   `localStorage`, so you only need to do this once. **The page defaults to
   its normal opaque background** (`avatar-window` / `var(--bg)`); if you
   skip this step the window will look opaque no matter what the Tauri side
   is doing, and that is not evidence of anything.
4. With backdrop set to "transparent": look at the area around the VRM
   model. You should see **whatever is actually behind the window on your
   desktop** (wallpaper, other windows, desktop icons) rather than a solid
   black rectangle or any other flat colour.
5. To be sure it's real compositing and not a static screenshot-like
   artifact, move another window (or drag a colourful image) behind the
   mascot window and confirm the visible desktop content changes to match.
6. The model itself should still be interactive (`VrmStage`'s
   `interactive` prop -- click-drag orbits the camera) everywhere except a
   thin 10px strip along the very top edge, which is claimed by the
   drag-handle script (see below).
7. To close: there is no title bar. Use the tray icon (bottom-right of the
   Windows taskbar, may be under the "^" overflow chevron) -> Quit. Show/Hide
   are also there.

## If the canvas comes out black (or otherwise wrong)

In order of how much they preserve the original goal:

1. **Check step 3 above first.** By far the most likely "failure" is
   forgetting to toggle the page's own backdrop to "transparent" -- that is
   a page-side setting, not a Tauri-side one, and this app does not force it
   for you (it can't, without editing `web/`, which is out of scope here).
2. If backdrop is confirmed "transparent" and the window is still opaque
   black: this is the actual risk the spike was checking for. Two fallbacks,
   easiest first:
   - **Chroma-key backdrop.** Toggle the same control to "chroma" instead of
     "transparent" (`avatar-window--chroma`, solid `#00b140`). If that
     renders correctly (solid, evenly-coloured green, no black) while
     "transparent" doesn't, the WebGL canvas itself is fine and the bug is
     specifically in WebView2's per-pixel-alpha compositing path. The
     practical fallback is a chroma-keyed window: keep it opaque-green and
     key it out downstream (OBS chroma key, or similar), the same way
     capture-source setups already use this page.
   - **Opaque window.** Drop `transparent`/`background_color` entirely and
     ship a normal, non-see-through frameless window (still undecorated,
     always-on-top, tray-controlled). Loses the "sits directly on the
     desktop" effect but keeps everything else (port discovery, tray, the
     window shape itself).

## Dragging -- outcome: implemented, but UNVERIFIED

A frameless window needs an explicit way to move it. `tc-assistant2` solves
this by having its own (bundled, local) frontend call a `start_window_drag`
command on `mousedown`. Our page is different in kind: it's **remote**
content served by tc-npc, so Tauri does not inject its JS API into it by
default, and `web/` is out of bounds for this spike (other workers are
editing it concurrently), so adding a `data-tauri-drag-region` attribute to
the page's own markup was not an option.

What's actually implemented instead (`src/main.rs`):

- `capabilities/main.json` grants the `main` window's own IPC bridge to
  tc-npc's loopback origin via a `remote.urls` allowlist
  (`"http://127.0.0.1:*"`), scoped to exactly one permission
  (`core:window:allow-start-dragging`) -- nothing else is exposed to the
  remote page.
- `DRAG_HANDLE_SCRIPT` in `src/main.rs` is injected via
  `.initialization_script(...)`, entirely from the Rust side. It splices a
  thin (10px) `data-tauri-drag-region` strip along the top edge into the
  page's DOM once it loads. Tauri already ships a built-in script (see
  `tauri-2.11.2/src/window/scripts/drag.js` in the crate source) that
  listens for `mousedown` on any such element and calls `start_dragging` for
  us -- so this only had to create the element, not reimplement drag
  handling.
- The strip is kept to the top 10px rather than the whole window on
  purpose: `AvatarView.tsx`'s own hover-revealed controls sit inset from the
  top-right corner, and the VRM stage is itself interactive (mouse-drag
  orbits the camera), so claiming the full window would fight both instead
  of coexisting with them.

**Why this is flagged unverified rather than done:** this spike was told not
to launch the app to check the visual/transparency result, and that same
constraint means the drag path has never been exercised end-to-end either.
Two things about it carry real residual doubt:

- Whether Tauri's `remote.urls` matcher (which uses the
  [URLPattern](https://urlpattern.spec.whatwg.org/) standard) actually
  accepts a bare `*` for the *port* component the way it's written here.
  `cargo build` succeeding is good evidence the capability JSON is at least
  syntactically valid and the permission identifier is real (tauri-build
  compiles the ACL at build time and would fail loudly on a bad
  identifier) -- but that does not prove the pattern *matches* at runtime.
- Whether granting `remote` IPC access to a `127.0.0.1:*` origin behaves the
  way it's expected to when the actual port varies per run (tc-npc's port is
  only known at the mascot's own startup, not at compile time).

Per this spike's own instructions: **if dragging doesn't work, that is an
acceptable outcome.** The window still opens, sits wherever
`inner_size(360.0, 480.0)` and the OS/WebView2 default placement put it, and
every other piece (port discovery, transparency, tray Show/Hide/Quit) is
unaffected either way. If it needs to be diagnosed: open devtools on the
mascot window (there's no in-app toggle for it here, but
`WebviewWindowBuilder::devtools(true)` is a one-line addition) and check the
console for an IPC/permission-denied error on `mousedown` over the top
strip.

## Toolchain / system assumptions made

- **Windows target.** Everything above (the `background_color` alpha=0
  requirement in particular, per the doc comment on
  `WebviewWindowBuilder::background_color` in the installed
  `tauri-2.11.2` source) was reasoned about for Windows/WebView2
  specifically, since that's this machine's platform. Behaviour on
  macOS/Linux (WKWebView / WebKitGTK) was not considered.
- **WebView2 runtime** is assumed already installed (it ships with Windows
  11 by default, and `tc-assistant2` already depends on and builds against
  it on this same machine).
- **`tauri-cli` 2.11.2** is available via `npx tauri` (confirmed:
  `npx tauri --version` on this machine). Not required for the commands
  above (`cargo run`/`cargo build` are enough) but mentioned as an
  alternative since it's already present.
- **Icon**: `icons/icon.ico` is copied byte-for-byte from
  `tc-assistant2/src-tauri/icons/icon.ico` -- a known-good, already-embedded
  icon, reused only so `app.default_window_icon()` has something to hand the
  tray builder (see `setup_tray` in `src/main.rs`); it has no bearing on the
  actual spike question and can be swapped for a real mascot icon later.
- **Dependency pins** (`tauri = "2.5"`, `tauri-build = "2.2"`, `dirs = "5"`)
  were copied from `tc-assistant2/src-tauri/Cargo.toml`, which already
  builds successfully on this machine, rather than picked fresh -- resolved
  to `tauri 2.11.5` / `tauri-build 2.6.3` / `dirs 6.0.0` by Cargo (see
  `Cargo.lock` in this directory once generated).
- `cargo build` inside `mascot/` was run and **succeeded** (verified on this
  machine, ~2 minutes cold, a few seconds incrementally). `cargo metadata
  --no-deps` at the repo root was also checked and does **not** list
  `tc-npc-mascot`, confirming the workspace isolation holds.
