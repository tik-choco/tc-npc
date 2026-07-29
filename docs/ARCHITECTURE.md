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
| `CHAT_SILENT`        | `chat_silent`          |
| `CHAT_LOG`           | `chat_log`             |
| `SHORT_TERM_MEMORY`  | `short_term_memory`    |
| `LONG_TERM_MEMORY`   | `long_term_memory`     |
| `SPEECH`             | `speech`                |
| `VISION`             | `vision`                 |
| `ACTION`             | `action`                 |
| `TTS`                | `tts`                    |
| `SUSPEND`            | `suspend`                |
| `RESUME`             | `resume`                 |
| `VOICE_START`        | `voice_start`            |
| `VOICE_STOP`         | `voice_stop`             |
| `PERSON_SEEN`        | `person_seen`            |
| `PERSON_MEMORY`      | `person_memory`          |
| `PERSON_UPDATED`     | `person_updated`         |
| `PERSON_DELETED`     | `person_deleted`         |

These match `agent-common/pkg/bus` in the original Go suite one-for-one, so
existing documentation/mental models about "who publishes/subscribes what"
still apply — only the transport changed.

The four `PERSON_*` types have no Go equivalent — they're new for [person
memory](#person-memory):

| Type              | Topic          | Published by                              | Subscribed by                                                |
|-------------------|-----------------|--------------------------------------------|----------------------------------------------------------------|
| `person_seen`     | `topic::SENSE`  | npc-vision (`observe_person` tool call)     | npc-memory                                                      |
| `person_memory`   | `topic::MEM`    | npc-memory                                  | npc-talk (fills `{{person_memory}}`)                            |
| `person_updated`  | `topic::UI`     | npc-memory, npc-server (REST handlers)      | npc-server's `bus_forward` (relayed to WS clients as `person`)  |
| `person_deleted`  | `topic::UI`     | npc-server (REST `DELETE /api/people/:id`)  | npc-server's `bus_forward` (relayed to WS clients as `personDeleted`) |

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
`{{person_memory}}`, `{{persona}}`. `{{person_memory}}` is filled in the same
way as the other memory placeholders — empty string when there's nothing to
say — and templates written before it existed keep working unchanged (an
unknown placeholder is a no-op, not an error).

## Spoken-reply discipline (`npc-talk`)

A reply is read aloud (npc-speech turns `chat_response` straight into TTS) and
shown as a chat bubble, so it has to be *speech*, not prose. Two halves, both
in `crates/npc-talk/src/style.rs`:

- **`SPOKEN_REPLY_RULES`** — a built-in system message pushed after
  `talk.prompts` and before the affect state: no stage directions or
  narration, no markdown/emoji/speaker prefix, no line breaks, 1–2 short
  sentences, plus the turn-taking / grounding / closure rules adapted from
  the conversation-quality work in `tik-choco-lab/archives/agent-conversation`.
  It lives in the binary rather than in `config.json` because `talk.prompts`
  is *empty* by default — rules kept only in config are rules the NPC
  routinely runs without. `talk.style_rules: false` turns it off for a setup
  whose own prompts already cover this.
- **`sanitize_reply`** — applied to the finished reply (after any filter
  pass, before publish/history): strips parenthesized stage directions
  ("（ふっと視線を緩め…）"), asterisk roleplay markup, markdown line markers,
  and blank lines. Small local models leak these however the prompt is
  worded.

### Silence (`talk.allow_silence`, default on)

A turn may end with the NPC saying nothing — without it every utterance gets
an answer and a conversation can never actually end. Three routes into
[`is_silence`], all treated alike:

- the model emits the `<silence>` tag the reply rules define (conversation
  over, speech not addressed to the NPC, an unintelligible STT fragment);
- the affect model's closing state machine produces its wordless "……" (a
  farewell was already returned and the partner is just acking);
- the reply has nothing sayable left after sanitizing (it was all stage
  direction).

A silent turn publishes `agent:chat` / `chat_silent` `{reason, input}`
instead of `chat_response`: nothing is spoken, no bubble is drawn, and no
assistant turn goes into the history — but `chat_log` is still published with
an empty output, so npc-memory records what was said even though it went
unanswered. npc-server relays it as the WS `silent` frame, which is what
stops the web UI's typing indicator and leaves the muted "応答しませんでした"
line. `talk.allow_silence: false` restores the always-answer behavior.

[`is_silence`]: ../crates/npc-talk/src/style.rs

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

## Voice cascade (`npc-speech`)

The mic→reply loop ported from Go `agent-speech`:

```
mic (cpal capture thread) → resample to stt.input_sample_rate → RMS VAD
  → WAV segment → POST /audio/transcriptions → agent:sense / speech
  → npc-talk → LLM → agent:chat / chat_response
  → POST /audio/speech → cpal playback thread
```

Two things about this module differ from the others:

**It is always spawned.** `tts.enabled` / `stt.enabled` don't gate the spawn,
they gate the two audio threads, which `apply_config` starts and stops on
every `npc:config` `config_updated`. The same pass restarts the capture
thread when `speech.input_device` / `input_sample_rate` changes and the
playback thread when `speech.output_device` does, and leaves both alone
otherwise so an unrelated config save can't cut a reply off mid-word.
Endpoint, model, voice and the VAD knobs are read from the module's own copy
of the latest config, so nothing under `stt`/`tts`/`speech` needs an app
restart. (`ctx.config` remains the startup snapshot for every other module.)

**Barge-in.** While a clip is playing, mic audio never reaches the VAD —
transcribing the agent's own voice would have it talking to itself. With
`stt.barge_in` on (the default) the mic is still *measured*, against
`stt.input_threshold × stt.barge_in_factor`; sustained speech above that line
stops playback and publishes `agent:interrupt` / `interrupt`, and the
interruption is then picked up by the ordinary VAD path. The factor is above
1.0 because an open mic next to speakers hears the agent at roughly speech
level; raise it if replies interrupt themselves, or turn barge-in off to get
the mic muted for the whole clip. Priority clips (`agent:interrupt` / `tts`
— chimes, scheduler announcements) are exempt, matching the Go original's
`!a.isPriorityPlaying.Load()`.

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

## Person memory

Memories can be tied to an individual, not just to the conversation as a
whole. The data model, on-disk store, and wire-format conversion live in
`npc_core::person` (sibling of `npc_core::character`, same one-file-per-record
layout under `{data_dir}/people/{id}.json`, same atomic tmp+rename save):

- `Person { id, name, aliases, first_seen, last_seen, encounter_count,
  appearance, facts: Vec<PersonFact>, familiarity, source, notes }` and
  `PersonFact { text, source, created_at }` — the on-disk, snake_case shape.
- `people_dir` / `list_people` / `load_person` / `save_person` /
  `delete_person` — the store, all taking `data_dir: &Path`; ids are
  validated against `[A-Za-z0-9-]` before touching the filesystem, so a
  crafted id can never escape `{data_dir}/people/`.
- `new_person(name, source)` builds a fresh record (uuid v4 id,
  `first_seen`/`last_seen` = now, `encounter_count` = 1).
- `normalize_person_name` / `find_person` match a spoken or typed name
  against existing records: trim, fold full-width whitespace, strip a
  trailing Japanese honorific (さん/くん/ちゃん/様/氏/先生/せんせい), lowercase,
  then compare against `name` and every `aliases` entry.
- `person_to_wire(&Person) -> serde_json::Value` is the single place the
  on-disk snake_case shape is converted to the camelCase `PersonRecord` JSON
  shape used by REST/WS and the web UI; both npc-memory and npc-server call
  it rather than duplicating the field mapping.

### Data flow: chat → person

1. A speaker name arrives alongside speech — from `agent:sense`/`speech`'s
   optional `speaker` field, `agent:chat`/`chat_log`'s optional `speaker`
   field, or a scheduler/event's user name — and npc-memory resolves it to a
   `Person` via `find_person`, creating a new one (`source: "chat"` or
   `"event"`) if nothing matches.
2. npc-memory asks the LLM (`memory.people.extract_prompt`) to pull new
   facts about the people mentioned in the conversation out of the recent
   turns, as a JSON array of `{name, facts}`.
3. Extracted facts are appended to the matching person's `facts` (oldest
   dropped once `memory.people.max_facts` is exceeded), `last_seen` and
   `encounter_count` are updated, and the record is saved.
4. npc-memory publishes a short profile as `person_memory` on `topic::MEM`
   (`{person_id, name, content}`); npc-talk's `ChatEngine` stores it and fills
   `{{person_memory}}` in the prompt template exactly like
   `{{short_term_memory}}`/`{{long_term_memory}}` — empty when there's
   nothing to say.

### Data flow: vision → person

When `vision.person_detection` is enabled, npc-vision's VLM call can invoke
an `observe_person` tool once per capture cycle; the tool handler publishes
`person_seen` on `topic::SENSE` with `{name, appearance, note, source:
"vision"}` (`name` is often empty — the model frequently can't read a name
off the screen). If `memory.people.link_vision` is enabled, npc-memory
matches this against existing records by name first, then by appearance, and
either merges into the match (updating `appearance`/`last_seen`/
`encounter_count`) or creates a new record (`source: "vision"`). Since a name
can't always be recovered from the screen, npc-memory caps how many unnamed,
vision-only records it will create, rather than spawning a fresh `Person`
every time the VLM redescribes the same unidentified visitor slightly
differently.

Because both fact extraction and person detection go through an LLM/VLM,
this is best-effort: a sighting can go unmatched, a fact can be missed, and
a name can be misread — none of it is a guarantee of accurate recognition.

### Person-tagged long-term memory

Long-term memory chunks created while a resolved person is in context are
saved with `metadata.person_id` set to that person's id (empty when no
person was resolved). RAG search over the long-term store gives a small
relevance bonus to hits whose `metadata.person_id` matches the person
currently in context, so memories about the person you're talking to surface
ahead of otherwise-similar chunks about someone else. `GET /api/people/:id`
reuses the same `metadata.person_id` field to list a person's memories.

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
| `voice`           | `{active}` — position of the cascade voice loop's 開始/停止 switch; sent right after `hello` and broadcast on every change |
| `status`          | `{modules}`                                       |
| `error`           | `{message}`                                       |
| `inputAccepted`   | `{requestId}`                                     |
| `response`        | `{requestId, status: "done"\|"error", text?, message?}` |
| `person`          | `{person: PersonRecord}` — sent for both create and update, relayed from `person_updated` (`agent:mem`/`npc-server` handlers, see [Person memory](#person-memory)) |
| `personDeleted`   | `{id}` — relayed from `person_deleted`                |

### WebSocket, client → server

| Frame        | Fields                                            |
|--------------|-----------------------------------------------------|
| `input`      | `{text, speaker?}` — `speaker`, if present, is attached to the resulting `agent:sense`/`speech` payload so npc-memory can resolve it to a person |
| `command`    | `{text}`                                            |
| `interrupt`  | `{}` — published as `agent:interrupt` / `interrupt`; npc-speech emits the same envelope (with `{"source": "barge_in"}`) when the mic hears you talking over a reply |
| `suspend`    | `{}` — pauses TTS playback only, auto-expiring (`npc-speech: SUSPEND_TIMEOUT`) |
| `resume`     | `{}`                                                |
| `voiceStart` | `{}` — master switch for the cascade voice loop: mic → VAD/STT resumes feeding `agent:sense`/`speech`, and `chat_response` replies are spoken again. Unlike `suspend`/`resume` it never expires |
| `voiceStop`  | `{}` — mic stops reaching the STT pipeline and replies go unspoken; the cpal input stream stays open so restarting is instant |
| `event`      | `{kind, userName?, text?, amount?}` — `userName` is also carried through as `speaker` |

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
- `GET /api/memory` → `{shortTerm, longTerm}`; each `longTerm` entry now also
  carries `personId` (the chunk's `metadata.person_id`, or `""` if none —
  see [Person memory](#person-memory)).
- `GET /api/people` → `{people: PersonRecord[]}`, sorted by `lastSeen`
  descending.
- `POST /api/people` → body `{name, notes?}` creates a manual record
  (`source: "manual"`; if a matching name/alias already exists that existing
  record is returned instead) → `{person: PersonRecord}`.
- `GET /api/people/:id` → `{person: PersonRecord, memories: [{docId, text,
  createdAt}]}`, the person's `metadata.person_id`-tagged long-term memories,
  newest first, capped at 50.
- `PATCH /api/people/:id` → body `{name?, aliases?, notes?, appearance?}`
  updates only the fields sent → `{person: PersonRecord}`.
- `DELETE /api/people/:id` → `{ok: true}`.
- `POST`/`PATCH`/`DELETE` on `/api/people*` publish `person_updated` /
  `person_deleted` on `topic::UI` on success; an unknown `:id` is a 404
  `{error: "..."}`.
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
