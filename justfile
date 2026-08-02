# tc-npc developer recipes. Run `just <recipe>`.

# On Windows, run recipes with cmd.exe so no POSIX sh (Git Bash) is required.
set windows-shell := ["cmd.exe", "/c"]

# Type-check the whole workspace.
check:
    cargo check --workspace

# Build the whole workspace (debug).
build:
    cargo build --workspace

# Type-check with the mist feature (heavy: compiles mistlib's WebRTC stack from git).
check-mist:
    cargo check --features mist

# Build with the optional mist (mistlib P2P) feature enabled.
build-mist:
    cargo build --features mist

# Run the server with no GUI.
run:
    cargo run -p tc-npc -- serve --no-open

# Run the full desktop app: server plus main window and mascot overlay.
app:
    cargo run --features desktop -- app

# Run the mascot overlay alone, against a tc-npc already serving.
mascot:
    cargo run --features desktop -- mascot

# Attach the terminal UI to an already-running instance.
tui:
    cargo run -- tui

# The bundle is embedded into the Rust binary at compile time, so anything
# that ships has to run this first or it ships the previous one.
#
# Build the web UI.
web-build:
    cd web && npm install && npm run build

# Build the web UI then the Rust binary.
all: web-build build

# The web UI goes first because it is embedded into the binary at compile
# time, so building it second would ship the previous bundle. `--features
# desktop` is what lets the one resulting exe open its own windows rather
# than only serve.
#
# Ship this: one executable, four modes (`app` default, `mascot`, `serve`, `tui`).
release: web-build
    cargo build --release --features desktop

# For a machine with no display to put a window on — a server, or the
# roadmap's robot target. `serve` and `tui` still work; `app`/`mascot` exit
# with a message telling you to rebuild.
#
# Same, minus the desktop UI: no Tauri in the dependency graph at all.
release-headless: web-build
    cargo build --release

# Drop build artifacts no current build can reuse. Cargo appends
# content-hashed files to target/ and never collects the old ones, so a
# save-driven loop like `just watch` piles up generations: 484 incremental
# sessions accumulated here over 8 days, of which only the newest per crate is
# ever reused again. Sweeping costs nothing in rebuild time for exactly that
# reason — what it removes was already dead. Run it after a heavy watch
# session, or on a schedule.
#
# Needs `cargo install cargo-sweep`.
sweep:
    cargo sweep --installed
    cargo sweep --time 7

# Show what `just sweep` would remove, without removing it.
sweep-dry:
    cargo sweep --dry-run --installed
    cargo sweep --dry-run --time 7

# Uses cargo-watch's `-- <full command>` form: `-x "run -p tc-npc"` breaks under
# the cmd.exe shell above, which doesn't unquote what just passes it.
# `--no-open` keeps each restart from popping a new browser tab: the tab you
# already have reconnects over /ws by itself.
#
# Rebuild & rerun on Rust source changes (needs `cargo install cargo-watch`).
watch:
    cargo watch -w src -w crates -w Cargo.toml -- cargo run -p tc-npc -- serve --no-open

# Run `just run` in another terminal — /ws, /api and /healthz are proxied to
# the binary on 127.0.0.1:47950.
#
# Vite dev server with HMR for the web UI.
dev-web:
    cd web && npm run dev

# For when the UI has to be checked through the Rust binary itself rather than
# the Vite dev server (`just run` / `just watch` in another terminal). No Rust
# rebuild is needed to pick the result up: rust-embed serves web/dist from disk
# in debug builds, so a browser reload is enough. Skips the `tsc -b` that
# `just web-build` runs — use that (or `npx tsc --noEmit`) for the type check.
#
# Rebuild web/dist on every web/src change.
watch-web:
    cd web && npx vite build --watch

# Replay the labelled conversations through the affect model, dumping the
# frames the web UI would have received (see eval/emotion/CONTRACT.md).
eval-trace:
    cargo run -p npc-talk --example affect_trace

# Score the expression pipeline end to end: replay the labelled conversations
# through the affect model, then run the frames it produced through the real
# affect -> VRM expression mapping and print accuracy, the confusion matrix,
# and the dev/holdout gap. Run this after touching affect.rs's vocabulary or
# vrm-emotion.ts's scoring — neither can be judged by reading the diff.
eval-emotion: eval-trace
    cd web && npm run eval:emotion
