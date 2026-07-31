# tc-npc-mascot

Puts tc-npc's character on the desktop: a transparent, undecorated,
always-on-top window showing the NPC's VRM directly over whatever else is on
screen. Drag it where you want it; it comes back there next launch.

It has no logic of its own. It opens **one** window against tc-npc's
already-running `/#/avatar` page — the same chrome-free avatar route that a
browser or a capture source would open — and gets out of the way. If this app
grows chat, state, or settings, that belongs in tc-npc, not in the window
shell that displays it.

## Requirements

tc-npc must already be running and serving the avatar page. This app has no
NPC of its own; it only displays tc-npc's.

## Running it

```sh
cd mascot
cargo run --release      # or: cargo build --release && ./target/release/tc-npc-mascot.exe
```

`npx tauri dev` / `npx tauri build` also work from this directory, since
`tauri.conf.json` sets no `beforeDevCommand`/`beforeBuildCommand` to proxy
through — but plain `cargo run`/`cargo build` is simpler, as there is no
bundled frontend for the CLI to manage. `mascot/` is its own Cargo workspace
(see the comment atop `Cargo.toml`), so none of this touches or is touched by
the root `tc-npc` workspace build.

## Using it

- **Move it**: drag the top 10px of the window.
- **Resize it**: drag an edge.
- **Hide / show / quit**: the tray icon (bottom-right of the taskbar, possibly
  under the `^` overflow chevron). There is no title bar, so this is the only
  way to close it.
- **Change how it looks**: hover the window to reveal the avatar page's own
  controls (top-right) — framing, backdrop, captions. This app only sets their
  *first-run* defaults (see below); after that your choice wins and persists.

The window reads the port tc-npc actually bound to from
`~/.tc-npc/server-port.txt`, falling back to `47950` (tc-npc's own default) if
that file is missing — see `discover_port()` in `src/main.rs`. A stale port
just means the window shows tc-npc's "can't reach the server" state.

### Defaults this app sets on the page

The avatar page defaults suit a browser tab, not a desktop mascot, so
`PAGE_DEFAULTS_SCRIPT` (`src/main.rs`) seeds two `localStorage` keys **only
when they are unset**:

| key | page default | here | why |
|---|---|---|---|
| `…:backdrop` | `normal` | `transparent` | otherwise the window is an opaque plate and none of the transparency below is visible |
| `…:captions` | `strip` | `bubble` | the subtitle bar is a full-width plate hanging under the character; a balloon isn't |

These land in *this app's* WebView2 user-data folder, which is separate from
your browser's. A capture setup using the same page on chroma-key with a
subtitle strip is not affected.

## Where the window state is kept

`tauri-plugin-window-state`, restricted to `SIZE | POSITION`. The default flag
set also restores `decorations` and `visible`; this window's answer to both is
fixed in code, and a saved file shouldn't get a vote on it.

## The question this started as

This began as a spike asking one thing:

> Can a transparent, undecorated, always-on-top Tauri 2 window composite a
> three.js/@pixiv/three-vrm WebGL canvas correctly on WebView2 — or does the
> canvas come out as an opaque black rectangle?

The concern was real: `tc-assistant2` already proved a transparent Tauri
window works for **DOM content** (its mascot is a `<div>` with a
`background-image`), but nothing here had ever put a **WebGL canvas** in one,
and that combination is known to be fragile on WebView2 — the compositor can
flatten the canvas's alpha to opaque black.

**Answer: it composites correctly. No fallback is needed.**

### How that was measured

Not by eye. The window was parked at a known rect, hidden, and the screen
captured (baseline); then shown, and the same rect captured again. Every pixel
was classified:

```
total     : 172800   (360x480)
unchanged : 144161 (83.4%)   <- identical to the desktop behind it
blackened : 0      ( 0.0%)   <- the failure signature
other     : 28639  (16.6%)   <- the model, and the status pill
```

83.4% of the window was **bit-identical** to what was behind it, and text in
the window underneath was legible straight through the transparent region.
Nothing was blackened. The 16.6% that changed is the character's silhouette
plus the status pill.

Two things this settles beyond the window itself:

- **Dragging works.** A synthetic press-drag-release on the injected
  `data-tauri-drag-region` strip moved the window exactly with the cursor.
  That also confirms the open doubt about `capabilities/main.json`: the
  `remote.urls` allowlist (`"http://127.0.0.1:*"`) really does match at
  runtime with the port known only at startup, so the remote page can reach
  the one IPC command it is granted (`core:window:allow-start-dragging` —
  nothing else is exposed to it).
- **AR (roadmap 段6) has the same substrate question**, and it now has the
  same answer.

### The bug found on the way

The first thing this spike would have shown, had it been run as originally
written, was an **opaque window** — and that would have been read as the
compositing failure it was built to detect. It wasn't.

`.avatar-window--bare` stripped the background from the avatar page's own
`<div>`, but `:root` carries `background: var(--bg)` (`web/src/index.css`),
and a background on the *root* element propagates to the viewport canvas — it
paints the whole window regardless of what any descendant does. The page's
"transparent" backdrop was never actually transparent.

Fixed in `web/`: `AvatarView` now marks the document
(`:root[data-avatar-backdrop]`) and `styles/avatar.css` strips the root
background to match. Both rules are needed; either alone does nothing useful.

## Toolchain / platform notes

- **Windows / WebView2.** Everything above — the `background_color` alpha=0
  requirement in particular, per the doc comment on
  `WebviewWindowBuilder::background_color` in the `tauri` source — was
  reasoned about and verified for Windows specifically. Behaviour on
  macOS/Linux (WKWebView / WebKitGTK) has not been looked at.
- **Icon**: `icons/icon.ico` is copied byte-for-byte from
  `tc-assistant2/src-tauri/icons/icon.ico`, reused so `default_window_icon()`
  has something to hand the tray builder. Swap it for a real mascot icon
  whenever one exists.
- **Dependency pins** (`tauri = "2.5"`, `tauri-build = "2.2"`, `dirs = "5"`)
  were copied from `tc-assistant2/src-tauri/Cargo.toml` rather than picked
  fresh, since that already builds on this machine.
