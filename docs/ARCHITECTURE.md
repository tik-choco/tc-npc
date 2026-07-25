# tc-npc Architecture

`tc-npc` unifies the former Go `agent-talk` / `agent-memory` / `agent-speech`
/ `agent-vision` / `agent-action` / `agent-scheduler` services — previously
separate processes wired together over Redis pub/sub — plus a web UI server,
into a single Rust binary. **Redis is not used**: the pub/sub bus is now an
in-process `tokio::sync::broadcast` channel.

## Crate layout

```
tc-npc/                 root binary crate (src/main.rs) — CLI, wiring, startup
crates/
  npc-core/              bus, Module trait, config, character sheets — no I/O to external services
  npc-llm/                OpenAI-compatible API client (chat/stream/embeddings/STT/TTS)
  npc-talk/               chat/dialogue module               (ports agent-talk)
  npc-memory/             short/long-term memory module       (ports agent-memory)
  npc-speech/              STT/TTS + mic/speaker I/O module     (ports agent-speech)
  npc-vision/              screen capture + description module (ports agent-vision)
  npc-action/              movement/OSC module                 (ports agent-action)
  npc-scheduler/           scheduled announcements module      (ports agent-scheduler)
  npc-translate/           simultaneous interpretation module  (ports agent-speech's translation)
  npc-server/              HTTP/WebSocket server + embedded web UI host
```

`npc-talk`, `npc-memory`, `npc-speech`, `npc-vision`, `npc-action`,
`npc-scheduler`, and `npc-server` currently contain only stub
`module(&ModuleCtx) -> anyhow::Result<Box<dyn Module>>` constructors that
`bail!("... not yet implemented")`. `src/main.rs` calls each constructor for
modules enabled in config, logs a warning and continues if a module is
unavailable (which today means "always", since they're stubs), and always
attempts to start `npc-server` since that's how the web UI and mascot client
connect.

## The bus

`npc_core::bus::Bus` wraps a single `tokio::sync::broadcast::Sender<BusMessage>`
(capacity 256). Every module gets a clone via `ModuleCtx.bus` and can
`publish` or `subscribe`. The envelope shape is unchanged from the Go
services: `{"type": "...", "payload": ...}`.

### Topics (`npc_core::bus::topic`)

| Constant    | Value             |
|-------------|-------------------|
| `CHAT`      | `agent:chat`      |
| `MEM`       | `agent:mem`       |
| `SENSE`     | `agent:sense`     |
| `INTERRUPT` | `agent:interrupt` |
| `ACTION`    | `agent:action`    |

### Message types (`npc_core::bus::msg`)

| Constant             | Value                 |
|----------------------|-----------------------|
| `CHAT_RESPONSE`      | `chat_response`       |
| `CHAT_LOG`           | `chat_log`             |
| `SHORT_TERM_MEMORY`  | `short_term_memory`    |
| `LONG_TERM_MEMORY`   | `long_term_memory`     |
| `SPEECH`             | `speech`                |
| `VISION`             | `vision`                 |
| `ACTION`             | `action`                 |
| `TTS`                | `tts`                    |
| `SUSPEND`            | `suspend`                |
| `RESUME`             | `resume`                 |

These match `agent-common/pkg/bus` in the original Go suite one-for-one, so
existing documentation/mental models about "who publishes/subscribes what"
still apply — only the transport changed.

## The `Module` trait

```rust
#[async_trait]
pub trait Module: Send {
    fn name(&self) -> &'static str;
    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()>;
}

#[derive(Clone)]
pub struct ModuleCtx {
    pub bus: Bus,
    pub config: Arc<Config>,
    pub shutdown: CancellationToken,
    pub data_dir: PathBuf,
}
```

Each crate exposes a `pub fn module(ctx: &ModuleCtx) -> anyhow::Result<Box<dyn Module>>`
factory. `run` takes ownership of the boxed module and should run until
`ctx.shutdown` is cancelled (or return an error to abort early — the caller
logs and moves on rather than crashing the whole process).

## Config schema

Loaded from `--config <path>` or `./config.json` (a missing file just means
defaults); `.env` is loaded first via `dotenvy`, then a fixed set of
environment variables override the matching JSON fields. See
`config.example.json` for a full example and `crates/npc-core/src/config.rs`
for the authoritative field list/defaults. Top-level sections: `api`, `tts`,
`stt`, `talk`, `memory`, `vision`, `speech`, `action`, `vrc`, `scheduler`,
`translation`, `server`, `character`, `mist`. `Config::redacted_json()` masks every
`api_key` field for safe display in the web UI.

Prompt templates in `talk.prompts[].content` support placeholders:
`{{session_meta}}`, `{{short_term_memory}}`, `{{long_term_memory}}`,
`{{persona}}`.

## Scheduled actions

`scheduler.announcements[].actions[]` is the port of Go `agent-scheduler`'s
`redis_actions`: extra bus messages an announcement publishes when it fires,
independent of its `text` (an entry with no text and only actions is a
perfectly good "do something at 17:00"). Each entry is tagged on `kind`
(`npc_core::config::ScheduledAction`):

| `kind`     | Publishes                                            |
|------------|------------------------------------------------------|
| `speak`    | `agent:interrupt` / `tts` `{content, chime_file}`     |
| `action`   | `agent:action` / `action` `{content}` (NL → npc-action) |
| `command`  | `agent:action` / `command` `{text}` (CLI dispatcher)  |
| `chat`     | `agent:sense` / `speech` `{content}` (answered by npc-talk) |
| `suspend`  | `agent:interrupt` / `suspend`                         |
| `resume`   | `agent:interrupt` / `resume`                          |
| `raw`      | `{topic, type, payload}` verbatim — the literal Go form |

`npc_scheduler::fire_announcement` is the single place this happens, shared
by the clock and `POST /api/scheduler/test`.

## Simultaneous interpretation (`npc-translate`)

Ports the translation half of Go `agent-speech` (`handleTranslation` /
`handleAgentTranslation`), driven by `config.translation`:

- `mode: "off"` — nothing runs.
- `mode: "interpret"` — heard speech (`agent:sense` / `speech`) is translated
  and **npc-talk does not answer it**: npc-talk reads the same setting and
  drops speech input while this mode is on, so the NPC acts purely as an
  interpreter.
- `mode: "assist"` — normal conversation, with both heard speech and the
  NPC's `chat_response` translated as subtitles.

Targets are `target_language` (+ optional `target_language_2`), with
`auto_reverse` translating a reply *in* a target language back into
`source_language`. Results are published on `npc:ui` as `translation`
messages — one per target language, sharing an `id` with the "heard this
line" message published before them so the UI can group them — and
optionally mirrored to the VRChat chatbox over OSC (`translation.chatbox`).

Like npc-scheduler, npc-translate is always spawned and re-reads
`config.translation` from `npc:config` `config_updated`, so the mode and
languages can be switched from the web UI without a restart; the LLM
connection (`api.*`) is still startup-only.

## Characters

`npc_core::character` stores character sheets as JSON files under
`{data_dir}/characters/{id}.json` (`data_dir` = `~/.tc-npc/`).
`import_tc_town_export` parses a tc-town character export bundle
(`{"app":"tc-town","version":1,"kind":"character","characters":[...]}`) into
`Character`s; `persona_prompt` renders a `CharacterSheet` into the Japanese
sectioned system-prompt format tc-town itself uses (skipping empty
sections).

## WS / REST protocol (implemented by `npc-server`)

### WebSocket, server → client (`/ws`)

| Frame            | Fields                                          |
|-------------------|--------------------------------------------------|
| `hello`           | `{version, modules, character}`                  |
| `chat`            | `{role: "user"\|"assistant", text, ts}`          |
| `ttsLine`         | `{text, translations?}`                          |
| `sense`           | `{kind: "vision"\|"speech", text, ts}`            |
| `translation`     | `{id, source: "user"\|"agent", original, lang, text, reversed, ts}` |
| `memory`          | `{kind: "short"\|"long", text}`                   |
| `actionLog`       | `{text}`                                          |
| `position`        | `{x, y, heading}`                                 |
| `volume`          | `{level}`                                         |
| `status`          | `{modules}`                                       |
| `error`           | `{message}`                                       |
| `inputAccepted`   | `{requestId}`                                     |
| `response`        | `{requestId, status: "done"\|"error", text?, message?}` |

### WebSocket, client → server

| Frame        | Fields                                            |
|--------------|-----------------------------------------------------|
| `input`      | `{text}`                                            |
| `command`    | `{text}`                                            |
| `interrupt`  | `{}`                                                |
| `suspend`    | `{}`                                                |
| `resume`     | `{}`                                                |
| `event`      | `{kind, userName?, text?, amount?}`                 |

### REST

- `GET /healthz` → `{ok: true}`
- `GET /api/state`
- `GET /api/config` (redacted) / `PUT /api/config`
- `GET /api/characters`
- `POST /api/characters/import` (tc-town export JSON body)
- `POST /api/characters/{id}/activate`
- `POST /api/scheduler/test` → `{index?, text?, chime_file?, actions?}` fires
  one announcement immediately (ignoring the clock and `scheduler.enabled`)
  via `npc_scheduler::fire_announcement`; responds `{ok, fired, spoke,
  actions}`, where `fired: false` means the entry is a no-op (neither text
  nor actions).
- `GET /` and all non-`/api`/`/ws` paths → embedded `web/dist` static files,
  with SPA fallback to `index.html`.

## Ownership

This scaffold (workspace, `npc-core`, `npc-llm`, CLI skeleton, stub crates,
docs, example config) is the foundation. Other workers implement the real
logic inside `npc-talk`, `npc-memory`, `npc-speech`, `npc-vision`,
`npc-action`, `npc-scheduler`, and `npc-server` (including the `web/`
frontend, which this scaffold intentionally does not create).

## mist (optional feature)

`npc-mist` (`crates/npc-mist`) is an optional, feature-gated module that
connects tc-npc to the [mistlib](https://github.com/tik-choco-lab/mistlib)
P2P network (MPL-2.0). It's excluded from the default build — `cargo check
--workspace`/`cargo build --workspace` never touch it. Building with
`cargo build --features mist` (or `just check-mist` / `just build-mist`)
pulls in `mistlib-core`/`mistlib-native` as git dependencies pinned to a
specific commit and compiles the module in; it's a heavy build (mistlib
bundles a WebRTC stack). `src/main.rs` only spawns the module when both the
`mist` feature is enabled at compile time *and* `config.mist.enabled` is
true at runtime (see `config.mist` in `crates/npc-core/src/config.rs`:
`enabled`, `signaling_url`, `room_id`).

When running, it:

- Generates (or loads) a persistent node id at
  `{data_dir}/mist-node-id.txt` (`tc-npc-<8 hex chars>`).
- Initializes the mistlib engine — falling back to mistlib's own default
  signaling URL (`mistlib_core::config::Config::new_default()`) when
  `config.mist.signaling_url` is empty — and joins a single room:
  - `config.mist.room_id` if set: a plain presence/messaging room, with no
    special handling of its traffic yet.
  - Otherwise, the public tc-town character catalog room
    (`tc-town-character-catalog-v1`): entries broadcast there are fetched
    via mist content storage (`storage_get`), parsed as tc-town's
    `CatalogPayloadV1`, converted to an `npc_core::Character`, and saved
    under `{data_dir}/catalog/{id}.json` — deliberately separate from
    `{data_dir}/characters/`, since these are *discovered* characters an
    operator hasn't chosen to import yet. Each discovery logs at info and
    publishes an `npc:ui`/`action_log` bus message so it shows up in the web
    UI's log.
  - mistlib-native's engine, at the pinned commit this crate depends on,
    tracks only **one** joined room per process, so setting
    `config.mist.room_id` disables catalog discovery for that run (logged
    as a warning) — see the module doc comment in
    `crates/npc-mist/src/engine.rs` for the underlying constraint.
- Leaves the room and drops the engine cleanly on `ctx.shutdown`.

Catalog entries are trusted without verifying tc-town's did:key signature
(`CatalogEntryWire.signature`) — there's no DID identity/crypto story on the
tc-npc side yet; see the `NOTE (v1)` comment in
`crates/npc-mist/src/catalog.rs`.
