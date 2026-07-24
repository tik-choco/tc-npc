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

# Run the tc-npc binary.
run:
    cargo run -p tc-npc

# Build the web UI (owned by another crate/worker; requires web/ to exist).
web-build:
    cd web && npm install && npm run build

# Build the web UI then the Rust binary.
all: web-build build

# Release build (web UI first — it's embedded into the binary at compile time).
release: web-build
    cargo build --release

# Rebuild & rerun on Rust source changes (requires `cargo install cargo-watch`).
watch:
    cargo watch -w src -w crates -w Cargo.toml -x "run -p tc-npc"
