# LLM configuration

tc-npc selects raw model IDs directly from enabled providers. There are no
runtime presets and no temperature parameter. The Rust config is the source
of truth for the Connections / Tasks / Sharing settings UI.

```json
{
  "providers": [
    {
      "id": "http",
      "label": "My endpoint",
      "base_url": "http://localhost:8000/v1",
      "api_key": "",
      "enabled": true,
      "models": ["chat-model", "embedding-model"]
    },
    {
      "id": "room",
      "label": "Team",
      "base_url": "mist-network://team-room",
      "api_key": "",
      "enabled": true,
      "models": ["remote-model"],
      "provide": true,
      "shared": [{ "provider_id": "http", "model": "chat-model" }]
    }
  ],
  "default_ref": { "provider_id": "http", "model": "chat-model" },
  "talk": { "reasoning_effort": "none" },
  "translation": {
    "model_ref": { "provider_id": "room", "model": "remote-model" },
    "reasoning_effort": "medium"
  },
  "memory": {
    "embedding_ref": { "provider_id": "http", "model": "embedding-model" }
  },
  "tts": {
    "model_ref": { "provider_id": "http", "model": "speech-model" },
    "voice": "speaker",
    "speed": 1.0
  },
  "stt": {
    "model_ref": { "provider_id": "http", "model": "transcription-model" }
  },
  "mist": {
    "ai_base_url": "http://127.0.0.1:6478/v1",
    "cli_path": "mistl",
    "instance": "work",
    "state_dir": "C:/path/to/mistl-state"
  }
}
```

These model names are placeholders. Replace them with your endpoint's raw IDs.
Each of `talk`, `memory`, `vision`, `action`, `translation`, `tts`, and `stt`
accepts an optional `model_ref`; `memory.embedding_ref` selects embeddings.
Unset refs inherit `default_ref`. Chat tasks carry `reasoning_effort`, falling
back to `api.reasoning_effort` when empty. `none` is an explicit value.
TTS voice/speed and STT microphone/VAD settings remain in their sections.

Providers default to `enabled: true`. Disabling preserves assignments. A task
whose provider is disabled falls back to a usable default at runtime without
rewriting its ref. A missing provider, blank model, or unusable default errors;
tc-npc never selects the first provider or cached model. Empty HTTP API keys
stay empty and do not inherit another provider's credentials.

`models` and optional `models_fetched_at` are caches, not allowlists. Provider
entries also accept `provide` and `shared` refs. The Sharing tab edits these
per-room flags and HTTP model refs; mistl applies them through external registration.

## Rooms through mistl

Run a mistl daemon implementing the external registration API (SPEC N1/N2)
and its `ai serve` room-scoped API (M1/M3). tc-npc registers its enabled rooms
automatically at server start and after a successful config save. It invokes
only `ai external apply --owner tc-npc`, `remove --owner tc-npc`, and
`get --owner tc-npc`; it never reads or edits mistl's configuration.

`mist.cli_path` defaults to `mistl` on PATH. If it is missing, tc-npc tries
`%LOCALAPPDATA%/Programs/mistl/mistl.exe`. An explicit executable path disables
that fallback. Optional `mist.instance` and `mist.state_dir` are passed as
separate global arguments (`--instance` / `--state-dir`), without a shell.
Omit them to use mistl's default instance. JSON travels over stdin/stdout;
each operation has a 10-second timeout and the child is killed on cancellation.

Registration replaces everything owned by `tc-npc`. Enabled rooms have
`consume: true` and their configured `provide` flag. Disabled rooms are omitted
so they neither join nor provide; their saved flags/refs remain intact. Only
enabled HTTP providers referenced by usable room shares are registered (with
their real keys); Room refs, missing/disabled providers and blank model refs
are excluded from shares. Providers and rooms are sorted deterministically;
shared refs keep their first-occurrence order because the first ref wins when
providers share the same raw model ID. With no enabled rooms, tc-npc removes
its registration. mistl owns merging with other registrations and its own
settings; a room disabled by the mistl user remains disabled and may produce
a warning.

Sync runs in the background and never blocks saving or server startup. Saves
are serialized with the latest config winning over older in-flight syncs.
Failures are logged and exposed via `GET /api/mist/sync` (`pending`, `applied`,
`error`, `warnings`, `updated_at`, `generation`); another save or server start
retries. `GET /api/mist/rooms` returns the `tc-npc` registration's applied rooms,
timestamp and live status (`joined`, `providing`, `peers`, `models`) from
external get, omitting provider credentials. CLI failures return HTTP 502.
While AI settings are open, the UI reads this live state for room status dots,
model lists and applied/pending Sharing state, and displays sync errors/warnings.

Room refs route to `{mist.ai_base_url}/rooms/{URL-encoded-room}/...`, with the
raw model ID and no API key. Chat (streaming or non-streaming) includes the
task's `reasoning_effort`, including explicit `none` and unknown values. mistl
uses `llm_request` for text chat and its OAI tunnel for image parts such as OCR.
Models and voice requests use the same room path; `network-auto` can defer
voice model selection to that room. The M3 contract does not define a
room-scoped embeddings route; use an HTTP provider for embeddings.
`mist.ai_base_url` defaults to `http://127.0.0.1:6478/v1`; set it to the actual
serve URL when using another port/instance. An HTTP provider pointing directly
at mistl's default `/v1` routes continues to work. `mist.enabled` controls the
optional native integration and is not required for registration or this bridge.

## Migration

On load, `presets`, `default_preset_id`, task `preset_id` and
`memory.embedding_preset_id` are consumed once, then omitted from saved JSON.
The same normalization runs before the REST config endpoint saves a document.
New refs and task reasoning efforts take precedence. Otherwise refs come from
the old preset, and chat tasks inherit its reasoning effort. Manual HTTP preset
models populate the provider cache; Room preset models are not mirrored.
Unresolved legacy IDs remain unusable assignments rather than adopting the
default silently. Migration is idempotent and saves only when it changes data.

Older direct `api.*`, TTS/STT and vision endpoint/model fields also migrate
when no providers or refs have been configured. Distinct endpoint or credential
pairs become distinct providers, retaining the previous per-use targets. These
direct fields remain readable for compatibility; runtime requests use refs.

Environment overrides apply after migration and are not written into the
migration save. `OPENAI_API_BASE_URL`, `OPENAI_API_KEY`, and `OPENAI_MODEL`
override the default target; `OPENAI_EMBEDDING_MODEL` overrides the embedding
ref. TTS/STT endpoint overrides create separate in-memory providers so voice
changes do not alter the chat connection. For a Room target, a URL override
changes the local mistl bridge URL and API keys are ignored.

## 日本語

モデルは `default_ref` と各タスクの `model_ref` で直接指定します。埋め込みには
`memory.embedding_ref` を使います。未指定は既定を継承します。接続先の無効化は
参照を変更せず、実行時に利用可能な既定へ切り替えます。接続先が見つからない、
モデルが空、既定が使えない場合はエラーです。temperature は送信しません。
旧プリセットと ID は初回読み込み時に移行して保存から除外します。
ルームは `mist-network://<room>` で指定し、`mistl ai serve` を経由します。
サーバー起動・設定保存時に `ai external apply/remove --owner tc-npc` で
有効なルームと HTTP の共有モデルを自動登録します。mistl の設定ファイルは読み書きしません。
`mist.cli_path`、任意の `mist.instance` / `mist.state_dir` で CLI を指定できます。
同期は非同期で、失敗しても保存を妨げません。接続先・提供タブに実際の接続状態、適用状態、
同期エラーを表示します。リクエストは `/rooms/{ルームID}/...` に送信し、タスクの
`reasoning_effort` を維持します。埋め込みには HTTP 接続先を使用してください。

## 中文

使用 `default_ref` 和各任务的 `model_ref` 直接选择模型；嵌入模型使用
`memory.embedding_ref`。未设置的引用继承默认值。禁用连接不会改写引用；运行时
会回退到可用的默认模型。连接不存在、模型为空或默认模型不可用时会报错。
请求不发送 temperature。旧预设及其 ID 在首次加载时迁移，之后不再保存。
房间使用 `mist-network://<room>`，通过 `mistl ai serve` 访问。服务器启动和保存设置时，
会调用 `ai external apply/remove --owner tc-npc` 自动注册启用的房间和 HTTP 共享模型，
不会读写 mistl 配置文件。可设置 `mist.cli_path` 及可选的 `mist.instance` / `mist.state_dir`。
同步在后台运行，失败不会阻止保存；连接和共享选项卡显示实际连接状态、应用状态及同步错误。
请求通过 `/rooms/{房间ID}/...` 路由，并携带任务的 `reasoning_effort`。
嵌入模型请使用 HTTP 连接。
