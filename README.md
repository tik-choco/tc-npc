# tc-npc

A single-binary Rust CLI/server that runs an AI NPC (mascot). Chat, memory,
speech, vision, VRChat avatar control, and scheduled announcements all run as
modules inside one process, talking to each other over an in-process bus
(`tokio::sync::broadcast`) instead of the original Go microservices +
Redis pub/sub setup. A local Web UI (default `http://127.0.0.1:47950`) is
served on startup.

日本語版は [README.ja.md](README.ja.md) を参照してください。

## Features

- **Chat**: OpenAI-compatible LLM chat agent with short/long-term memory and
  persona injected into the prompt template. Includes an affect/emotion drive
  model and automatic conversation closing (`talk.affect`).
- **Memory**: short-term summarization plus a chunked, embedded long-term
  vector store (RAG). Also tracks per-person records (`memory.people`) built
  from speaker names and, optionally, vision.
- **Speech**: VAD-segmented mic input via OpenAI-compatible STT, and
  OpenAI-compatible TTS playback. Disabled by default.
- **Vision**: periodic screen/window capture described by a VLM. Disabled by
  default.
- **Action**: VRChat-compatible OSC avatar control, position/route
  management, and natural-language-to-command generation. Disabled by
  default.
- **Scheduler**: timed announcements (TTS + chime) that can also fire actions
  (commands, chat input, suspend/resume, raw bus messages). Disabled by
  default.
- **Translation**: `interpret` (translate-only) and `assist` (chat +
  translation) modes, up to two target languages. Disabled by default.
- **Web UI**: a Preact app (chat/character/people/speech/vision/action/
  schedule/translation/settings tabs) bundled into the binary.
- **Character import**: loads tc-town character-export JSON as a persona
  sheet.
- **VRM avatar**: drop a `.vrm` into `~/.tc-npc/vrm/` (or add it from the
  キャラ tab) and the チャット tab can put the model front and centre — idle
  motion, blinking, an expression driven by the affect model, and a mouth
  that follows the loudness of the host's actual TTS playback. A character
  isn't required: with none set up the model is used on its own, and
  assigning one per character works too. `#/avatar` is the same avatar with
  no UI around it, for opening in a window of its own; its framing, backdrop
  and camera angle are remembered between reloads.
- **mist** (optional feature): connects to tc-town's character catalog room
  via mistlib (P2P). Disabled by default.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the modules,
bus, and protocol fit together, including a deeper look at person memory.

## Quick start

Prerequisites: Rust (stable), Node.js (for the Web UI build).

```bash
just all              # build web UI + Rust binary
tc-npc run             # first run
```

Or manually:

```bash
cd web && npm install && npm run build
cd ..
cargo build --release
```

The Web UI opens at `config.server.addr` (default `http://127.0.0.1:47950`).
Pass `--no-open` (or set `config.server.auto_open: false`) to skip
auto-opening a browser tab — useful with `just watch`, which restarts the
binary on every Rust source change without spawning a new tab each time.

## Tests

```bash
cargo test --workspace     # Rust
cd web && npm test         # Web UI (vitest)
```

## Desktop mascot (experimental)

`mascot/` is a thin Tauri shell that opens tc-npc's `#/avatar` page in a
transparent, undecorated, always-on-top window — the avatar on your desktop
rather than in a browser tab. It holds no logic of its own: it finds the
running server (via `~/.tc-npc/server-port.txt`) and points a window at it.

It is a **spike**, deliberately kept out of the main build (its own Cargo
workspace, like `crates/npc-mist`), because one question is still unanswered:
whether WebView2 composites a WebGL canvas correctly in a transparent window.
See `mascot/README.md` for how to run it and what to look for.

## Configuration

```bash
cp config.example.json config.json
```

`config.json` is read by default (or pass `--config <path>`). Secrets go in
`.env` (see `.env.example`), loaded before config and used to override a few
JSON fields via environment variables.

**Never commit API keys.** `config.json` and `.env` are already
`.gitignore`d.

LLM/TTS/STT endpoints are OpenAI-compatible; defaults point at a local
inference server (e.g. Ollama). See the comments in `config.example.json`
for the full list of sections and fields.

If your OpenAI-compatible endpoint uses a private-CA certificate (common on
institutional networks) and you can't install the root CA into the OS trust
store, set `TC_NPC_CA_BUNDLE` to a PEM bundle path, or `TC_NPC_INSECURE_TLS=1`
as a last resort. This is the fix for an LLM probe failing with
`invalid peer certificate: UnknownIssuer`.

## CLI

| Command | Description |
|---|---|
| `tc-npc run` | Start the agents and Web server (default when no subcommand is given) |
| `tc-npc import <path>` | Import a character from a tc-town export JSON |
| `tc-npc characters` | List imported characters |
| `tc-npc config-path` | Print the resolved config file path and data directory |

All subcommands accept `--config <path>`.

## License

This repository is [MIT licensed](LICENSE). The optional `mist` feature
depends on mistlib (MPL-2.0), fetched as an external dependency and not
included in this repo's code.
