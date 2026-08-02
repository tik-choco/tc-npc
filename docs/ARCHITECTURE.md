# tc-npc Architecture

`tc-npc` unifies the former Go `agent-talk` / `agent-memory` / `agent-speech`
/ `agent-vision` / `agent-action` / `agent-scheduler` services — previously
separate processes wired together over Redis pub/sub — plus a web UI server,
into a single Rust binary. **Redis is not used**: the pub/sub bus is now an
in-process `tokio::sync::broadcast` channel.

## Role

tc-npc is the runtime, not the author: it takes a character already authored
elsewhere and actually runs it, but has no persona editing, growth interviews,
or publishing of its own. Characters only flow in — via
`import_tc_town_export` (see [Characters](#characters)) or discovery in the
P2P catalog room (see [mist](#mist-optional-feature)) — never back out.

Where a character is embodied is a difference of actuator backend and display
mode, not a different application. VRChat avatar control over OSC
(`npc-action`) is today's backend, wired in behind the same `Module`/bus
structure as every other module described below; an additional actuator
backend is implemented the same way, not as a fork of tc-npc.

The LLM connection is likewise a configured endpoint, not a dependency:
`api.base_url` (`crates/npc-core/src/config.rs`) can point at any
OpenAI-compatible server — including a locally resident one — but nothing
here assumes that server exists. tc-npc runs the same way against a plain
OpenAI-compatible API with none of that present.

## Crate layout

```
tc-npc/                 root binary crate (src/main.rs) — CLI, wiring, startup
  src/desktop/           Tauri windows (`app`/`mascot`) — only compiled with --features desktop
crates/
  npc-core/              bus, Module trait, config, character sheets — no I/O to external services
  npc-llm/                OpenAI-compatible API client (chat/stream/embeddings/STT/TTS)
  npc-talk/               chat/dialogue module               (ports agent-talk)
  npc-memory/             short/long-term memory module       (ports agent-memory)
  npc-speech/              STT/TTS + mic/speaker I/O module     (ports agent-speech)
  npc-vision/              screen capture + description module (ports agent-vision)
  npc-action/              movement module, VRChat/OSC body    (ports agent-action)
  npc-scheduler/           scheduled announcements module      (ports agent-scheduler)
  npc-translate/           simultaneous interpretation module  (ports agent-speech's translation)
  npc-server/              HTTP/WebSocket server + embedded web UI host
  npc-tui/                 terminal UI — a *client* of npc-server, not a module
```

Every crate above exposes the same constructor,
`module(&ModuleCtx) -> anyhow::Result<Box<dyn Module>>`, and all of them are
implemented. `src/main.rs` calls the constructor, logs a warning and
continues if one returns `Err` — a module that can't start is not a reason to
take the rest of the process down with it.

Which ones get spawned is not simply "whatever config enables", because two
different things are being expressed:

- **Spawned only when enabled in config**: `npc-talk`, `npc-memory`,
  `npc-vision`, `npc-action`, and (behind the `mist` feature) `npc-mist`.
  Turning these on takes a restart.
- **Always spawned, idling when their feature is off**: `npc-speech`,
  `npc-scheduler`, `npc-translate`, and `npc-server`. These watch the
  `npc:config` bus topic and pick up changes made from the web UI live.
  Gating their *spawn* on the startup config is what made the voice toggles
  and schedule edits appear broken until a restart, which reads as the
  feature being broken rather than merely deferred — hence the split.

`npc-server` is in the second group for a further reason: it is how the web
UI, the desktop windows and the TUI all connect, so it has to be running
regardless of what else is.

## One binary, four modes

There is exactly one executable. What it does is chosen by subcommand, not by
which artifact you shipped:

| Mode | Owns a server? | Needs `--features desktop`? |
|---|---|---|
| `app` (default) | yes | yes |
| `mascot` | no — attaches to one | yes |
| `serve` (alias `run`) | yes | no |
| `tui` | no — attaches to one | no |

Two consequences are worth stating outright, because both are easy to break:

**Tauri must never reach a headless build.** The roadmap's AR and physical-
robot targets, and any server deployment, have no display to put a window on.
`tauri`/`tauri-plugin-window-state`/`tauri-build` are therefore optional
dependencies behind the `desktop` feature, and `src/desktop/` is declared
behind the same `cfg` — the same shape `crates/npc-mist` uses for the `mist`
feature, for the same reason. `cargo tree -e normal` with no features must
report zero `tauri` lines; that is the invariant, and it is cheap to check.
Without the feature, `app`/`mascot` return a plain error telling you to
rebuild, rather than failing to compile.

**Tauri's event loop owns the main thread on Windows.** That is why
`src/main.rs` has no `#[tokio::main]`: the runtime is built by hand, handed to
Tauri via `tauri::async_runtime::set`, and the modules are started on it
before `app` blocks on the event loop. `serve` builds the same runtime and
just `block_on`s instead. `start_modules` is the shared startup path, so the
two modes cannot drift apart in what they bring up.

`npc-tui` is a **client**, deliberately: it holds no bus, no modules, and no
`npc-core` dependency, and reaches tc-npc only over the HTTP/WebSocket API the
web UI already uses. That is what lets `tc-npc tui` attach to a tc-npc running
on another machine through an SSH port-forward. It learns the port the same
way any out-of-process client must — `~/.tc-npc/server-port.txt`, which
records the port actually bound, since `bind_with_retry` may have moved off
the configured one.

### The actuator seam (`npc-action`)

`npc_action::actuator::Actuator` is where "decide to move" stops and "move
this particular body" starts — four calls (`vertical`, `horizontal`,
`look_horizontal`, `jump`), axis values clamped to `-1.0..=1.0`. `Controller`
(timed primitives), `Navigator` (dead-reckoned metres/degrees), `Autopilot`
(routes) and the command dispatcher all hold an `Arc<dyn Actuator>` and name
no concrete body; `VrcClient` (`osc.rs`) is the one implementor today, and
`ActionModule::run` is the single place it is constructed. **An additional
body is a second implementor swapped in there** — see [Role](#role).

The trait is also what makes this layer testable: driving an axis used to
require a real UDP socket, so the code above it was only ever tested for its
arithmetic. `actuator::test_support::RecordingActuator` records calls in
order, which is how `controller.rs`'s tests pin down the contract that
matters most — an axis stays where it was put, so **a cancelled hold must
still zero it** or the body walks on after the command that started it was
cut short.

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
| `CHAT_ERROR`         | `chat_error`           |
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

A character id lands in a file path, so `load_character`/`save_character`
reject one containing a separator, `..`, or `:` — `PUT
/api/characters/{id}/avatar` puts the id straight into a write path.

**Switching characters is live.** `POST /api/characters/{id}/activate`
publishes `config_updated` on `npc:config`; npc-talk's existing subscriber
(`npc_talk::module`) compares `character.active_id` against the one it last
applied, and on a change reloads the sheet, re-renders it with
`persona_prompt`, and installs it on the engine via `set_persona`. The
persona lives on the engine's `ChatState`, behind the same mutex a chat turn
holds for its whole duration, so a switch can never tear the persona out
from under a turn already in flight — it lands on the next one. Clearing
`active_id`, or a character whose file has gone missing, resets the persona
to none: continuing to role-play someone the operator just deactivated is
the worse failure. Conversation history and the per-partner affect state are
deliberately left alone (wiping them would reintroduce exactly the
disruption that removing the restart was meant to avoid).

### Avatars (VRM)

A character may carry `avatar: {kind: "vrm", file}`, naming a `.vrm` in the
model folder `{data_dir}/vrm/` (`npc_core::vrm`). **The folder is the
library**: tc-npc runs on the operator's own machine, so a model copied in
by hand is available immediately, and `POST /api/vrm/{file}` is only a way
to do that copy from the browser. This is deliberately unlike tc-town, which
has no server to write files with and therefore keeps its models in
IndexedDB; the two libraries are separate (they aren't even the same origin).

The reference is by **file name**, not content hash, so it is stable across a
replaced file and survives a deleted one: a character pointing at a model
that is not in the folder is not an error anywhere — the web UI falls back to
the plain initial glyph. `import_tc_town_export` therefore also carries a
tc-town VRM avatar's `fileName` through, so dropping the same `.vrm` into the
folder attaches the avatar to an imported character with no further step.
Every name is validated (`validate_vrm_file_name`) before touching disk.

A character is not required. `config.character.avatar_file` names a model to
show when no active character supplies one, so a `.vrm` can be dropped in and
talked to without importing a tc-town export first — the NPC holds a
conversation with no character sheet either way. Resolution order is: the
active character's own `avatar`, else `character.avatar_file`, and in both
cases the reference is dropped if the file isn't in the folder
(`npc_server::ws::avatar_ref`).

Anything that changes that answer broadcasts an `avatar` frame, since `hello`
carries it only at connect time.

The model is rendered browser-side (three.js + `@pixiv/three-vrm`, see
`web/src/vrm/`, loaded as its own chunk via a dynamic `import()` so sessions
that never show an avatar don't download it), which is also where lip-sync is
driven from — see the `speaking` frame under
[Voice cascade](#voice-cascade-npc-speech).

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

## The face (affect → VRM expression)

The avatar's expression is not chosen by asking a model to label the reply.
It falls out of the affect state the chat turn already computed, in two
stages that live on opposite sides of the wire:

1. `npc-talk`'s `AffectState::update` (crates/npc-talk/src/affect.rs) folds
   the partner's utterance into 22 drives, published as the `affect` frame.
2. `decideEmotion` (web/src/lib/vrm-emotion.ts) reduces one frame to one of
   the six VRM standard expressions, which `VrmAnimator` eases in.

Three properties of that pipeline are easy to get wrong and impossible to
see in a diff, so they are worth stating:

- **The integrator is bounded.** A drive's step is scaled by its remaining
  headroom (`delta * (1 - level)` upward, `delta * level` downward). The
  original port integrated without bound, which put its fixed point at
  `base + delta/pull` — above 1.0 for every drive that ordinary conversation
  nudges each turn. Those drives pinned to 1.0 within a few turns and stopped
  carrying information at all, freezing both the face *and* the top-3 drives
  `to_prompt` feeds the persona. This is the one place affect.rs knowingly
  diverges from its TypeScript original; see the note in that file.
- **Wariness reaches the prompt but not the face.** A low `familiarity` adds
  to cortisol/noradrenaline so the NPC reads as guarded toward a stranger,
  which is right for the persona and wrong for the face — untreated it makes
  a polite first hello look like anger. `vrm-emotion.ts` subtracts an
  estimate of that contribution rather than affect.rs suppressing it, so the
  two consumers can disagree on purpose.
- **The decision is stateful.** `decideEmotion` takes the previous decision
  and applies a switch margin plus a minimum hold, because a per-frame argmax
  flickers whenever two emotions score closely. Consumers must therefore go
  through `useEmotion` (web/src/hooks/useEmotion.ts) and call it once per
  surface — two call sites are two independent state machines.

### Evaluating a change

Neither stage can be judged by reading it, so both are scored against
hand-labelled conversations. `just eval-emotion` replays
`eval/emotion/dataset.jsonl` through the real affect model, runs the frames
it produces through the real mapping, and prints accuracy, a confusion
matrix, and the dev/holdout gap. See
[eval/emotion/CONTRACT.md](../eval/emotion/CONTRACT.md) for the data format
and the reason the dataset is split.

Two numbers decide whether a change is an improvement. The first is the
**constant "always predict neutral" model**, printed beside every accuracy:
most turns in a real conversation genuinely are neutral, so a mapping can
post a respectable-looking score while having stopped expressing anything —
the vote-based mapping this replaced actually scored *below* that constant.
The second is the **dev/holdout gap**: vocabulary lists get tuned by people
who can read dev's phrasings, and a gap that opens up is that tuning failing
to generalise. `emotion-eval.test.ts` asserts on both.

## WS / REST protocol (implemented by `npc-server`)

The WS surface is deliberately a superset of the "extension API" a sibling
app (`tc-assistant2`) defines, so a client written against that spec works
here unchanged — see [Request correlation](#request-correlation) and the
`event`/ack rows below for the parts that exist purely for that
compatibility. Discovery works the same way too: the port actually bound is
written to `{data_dir}/server-port.txt` as decimal text, because
`bind_with_retry` may have had to fall back off the configured one and an
out-of-process client would otherwise have no way to find the server.

### Request correlation

A client's `input` is acked with `inputAccepted {requestId}`, and the
`response` that eventually answers it carries the same `requestId`. That id
has to survive a trip across three crates, since the reply is produced
asynchronously by a different module than the one that received the request:

1. `npc-server`'s `ws.rs` stamps the id onto the `agent:sense`/`speech`
   payload it publishes, as `request_id` — a sibling of the existing optional
   `speaker` field.
2. `npc-talk` reads it (`module::extract_request_id`) and carries it **as a
   call parameter**, not in `ChatState`: two turns can be queued at once, and
   shared state would let the second overwrite the first's id before it
   published — the exact bug this plumbing exists to prevent.
3. Whichever of the three outcomes ends the turn puts it back on the bus:
   `chat_response` (spoke), `chat_silent` (deliberately said nothing — this
   still has to release the waiting client), or `chat_error` (the turn failed
   outright, so neither of the others is coming). `bus_forward` turns each
   into a `response` frame.

A turn the NPC started by itself — a scheduled announcement, a remark
triggered by vision — has no requester, so the field is **absent** rather
than empty, and `bus_forward` reports `requestId: "-"`. `chat_error` isn't
published at all in that case: an error nobody is waiting on is a log line,
not a broadcast.

### WebSocket, server → client (`/ws`)

| Frame            | Fields                                          |
|-------------------|--------------------------------------------------|
| `hello`           | `{version, modules, character, avatar?}` — `avatar` is `{kind: "vrm", file}` when one resolves. It is a sibling of `character`, not a field on it: the NPC chats with no character sheet loaded, so the avatar resolves to the active character's own assignment *or* the standalone `config.character.avatar_file` (see [Avatars](#avatars-vrm)) |
| `chat`            | `{role: "user"\|"assistant", text, ts}`          |
| `ttsLine`         | `{text, translations?}` — one frame **per sentence**, published by npc-speech as it is about to speak that sentence (not once per reply). Correctly absent while TTS is off: nothing is being read aloud |
| `sense`           | `{kind: "vision"\|"speech", text, ts}`            |
| `translation`     | `{id, source: "user"\|"agent", original, lang, text, reversed, ts}` |
| `memory`          | `{kind: "short"\|"long", text}`                   |
| `actionLog`       | `{text}`                                          |
| `position`        | `{x, y, heading}`                                 |
| `volume`          | `{level}`                                         |
| `voice`           | `{active}` — position of the cascade voice loop's 開始/停止 switch; sent right after `hello` and broadcast on every change |
| `avatar`          | `{avatar}` — which VRM to display has changed (assigned/cleared, model added/removed, or a different character activated); `null` for none. Broadcast because `hello` carries the avatar only at connect time, so an already-open tab would otherwise not notice until reloaded |
| `speakingLevel`   | `{level}` — how loud the voice audible right now is (0..=1), published by npc-speech's playback thread roughly every 50ms while a clip plays and once as `0` when it ends. Drives how far the VRM avatar's mouth opens, so it tracks the real speech envelope instead of a fixed oscillation. Separate from `speaking`, which stays the authoritative on/off: a client that ignores this frame still animates correctly |
| `speaking`        | `{active}` — the synthesized voice is (or is no longer) audible on the host's speakers. Published by npc-speech's playback thread on each transition only, and used by the web UI to lip-sync the VRM avatar: the browser never receives the audio, and `ttsLine` fires when the *text* is ready (before synthesis is even requested), so neither can stand in for it |
| `status`          | `{modules}`                                       |
| `error`           | `{message}`                                       |
| `inputAccepted`   | `{requestId}`                                     |
| `eventAccepted`   | `{kind}` — ack for a client `event` |
| `interruptAccepted` | `{}` |
| `suspendAccepted`  | `{}` |
| `resumeAccepted`   | `{}` |
| `response`        | `{requestId, status: "done"\|"error", text?, message?}` — `requestId` is the id from the `inputAccepted` that started the turn, carried across the bus (see [Request correlation](#request-correlation)). `"-"` means the turn was self-initiated (a scheduled announcement, a vision remark) and nobody was waiting on it |
| `person`          | `{person: PersonRecord}` — sent for both create and update, relayed from `person_updated` (`agent:mem`/`npc-server` handlers, see [Person memory](#person-memory)) |
| `personDeleted`   | `{id}` — relayed from `person_deleted`                |

### WebSocket, client → server

| Frame        | Fields                                            |
|--------------|-----------------------------------------------------|
| `input`      | `{text, speaker?}` — `speaker`, if present, is attached to the resulting `agent:sense`/`speech` payload so npc-memory can resolve it to a person |
| `command`    | `{text}`                                            |
| `interrupt`  | `{}` — published as `agent:interrupt` / `interrupt`; npc-speech emits the same envelope (with `{"source": "barge_in"}`) when the mic hears you talking over a reply |
| `suspend`    | `{}` — holds incoming `input`/`event` at the WS layer until `resume` (bounded queue, oldest dropped on overflow) **and** pauses TTS playback. The playback half auto-expires after `npc-speech: SUSPEND_TIMEOUT`; the input gate does not, but is released if the client that suspended disconnects — so a crashed extension can't wedge the NPC, while an unrelated tab closing doesn't resume it |
| `resume`     | `{}`                                                |
| `voiceStart` | `{}` — master switch for the cascade voice loop: mic → VAD/STT resumes feeding `agent:sense`/`speech`, and `chat_response` replies are spoken again. Unlike `suspend`/`resume` it never expires |
| `voiceStop`  | `{}` — mic stops reaching the STT pipeline and replies go unspoken; the cpal input stream stays open so restarting is instant |
| `event`      | `{kind, userName?, text?, amount?, tier?, message?, rewardTitle?}` — `userName` is also carried through as `speaker`. Kinds: `follow`, `subscribe`, `resub`, `gift`, `cheer`, `raid`, `points`; an unknown kind falls back to `text` as its description. `amount` means whatever the kind says it means (months, bits, viewers, gift count) — it is deliberately **not** rendered as currency except for an actual donation |

### REST

- `GET /healthz` → `{ok: true}`
- `GET /api/state`
- `GET /api/config` (redacted) / `PUT /api/config`
- `GET /api/characters`
- `POST /api/characters/import` (tc-town export JSON body)
- `POST /api/characters/{id}/activate` — writes `character.active_id` and
  publishes `config_updated` on `npc:config`, the same way `PUT /api/config`
  does, so npc-talk swaps the persona live (see [Characters](#characters)).
  No restart.
- `PUT /api/characters/{id}/avatar` → body `{file}` points the character at a
  model in the VRM folder; `{file: null}` (or `""`) clears it. The model must
  already be in the folder — a name that isn't there is a 404 rather than a
  stored reference that could never resolve.
- `GET /api/vrm` → `{models: [{file, name, size}], dir, default}` — a listing
  of `{data_dir}/vrm/`, plus its absolute path (shown in the キャラ tab so the
  operator knows where to drop files directly) and the standalone default
  avatar's file name (`""` for none).
- `PUT /api/vrm/default` → body `{file}` sets `config.character.avatar_file`,
  the avatar used when no active character supplies one; `{file: null}` (or
  `""`) clears it. This is what lets a VRM be used with no character sheet at
  all. Registered before `/api/vrm/:file` so `default` isn't read as a model
  name.
- `GET /api/vrm/file/{file}` → the raw `.vrm` bytes
  (`model/gltf-binary`), for the browser's VRM loader. Sends a strong `ETag`
  built from the file's size and mtime and honours `If-None-Match` with a
  bodyless `304`, which for a 10-50 MB model is the difference that makes
  caching worth having. `Cache-Control` is `private, no-cache` — "revalidate
  every time", not "don't cache": with a validator in place the revalidation
  costs a `304`, and unlike a `max-age` it leaves no window in which
  overwriting a model (uploading the same file name) keeps serving the old
  bytes.
- `POST /api/vrm/{file}` (raw model bytes) copies a `.vrm` into the folder,
  replacing one of the same name. This route alone raises axum's body limit
  (`MAX_VRM_UPLOAD_BYTES`, 200 MB) — the 2 MB default won't pass a model.
- `DELETE /api/vrm/{file}`. Characters pointing at the deleted model keep the
  reference: restoring the file restores the avatar.
- `POST /api/scheduler/test` → `{index?, text?, chime_file?, actions?}` fires
  one announcement immediately (ignoring the clock and `scheduler.enabled`)
  via `npc_scheduler::fire_announcement`; responds `{ok, fired, spoke,
  actions}`, where `fired: false` means the entry is a no-op (neither text
  nor actions).
- `GET /api/affect/history?limit=N` → `{entries: [...]}`, oldest-first, each
  entry carrying the same camelCase fields as the `affect` WS frame. npc-server
  keeps a bounded rolling buffer of recent snapshots (filled in `bus_forward`
  beside the live broadcast, same trick as the `shortTerm` cache), because the
  browser's own affect history is live-frames-only and therefore empty on every
  page load — the 感情 tab's trend line seeds itself from this and then extends
  it with live frames, de-duplicating on `ts`.
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
specific tag (currently v0.6.0) and compiles the module in; it's a heavy
build (mistlib
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
  - mistlib-native's engine, at the pinned tag this crate depends on,
    tracks only **one** joined room per process, so setting
    `config.mist.room_id` disables catalog discovery for that run (logged
    as a warning) — see the module doc comment in
    `crates/npc-mist/src/engine.rs` for the underlying constraint.
- Leaves the room and drops the engine cleanly on `ctx.shutdown`.

Catalog entries are trusted without verifying tc-town's did:key signature
(`CatalogEntryWire.signature`) — there's no DID identity/crypto story on the
tc-npc side yet; see the `NOTE (v1)` comment in
`crates/npc-mist/src/catalog.rs`.
