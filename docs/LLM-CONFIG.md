# LLM configuration

tc-npc selects raw model IDs directly from enabled providers. There are no
runtime presets and no temperature parameter. The Rust config is the source
of truth; Phase 1 changes its schema and consumers, while the web settings
migration is a separate rollout task.

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
      "models": ["remote-model"]
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
  "mist": { "ai_base_url": "http://127.0.0.1:6478/v1" }
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
entries also accept `provide` and `shared` refs in the mistl reference shape;
tc-npc preserves them, but mistl owns network membership, discovery and sharing.

## Rooms through mistl

Start `mistl ai serve` and configure the corresponding enabled Room provider
and model in mistl. tc-npc routes Room refs to `mist.ai_base_url` (default
`http://127.0.0.1:6478/v1`), sends the raw model ID and no API key, and leaves
network routing to mistl. The existing configuration with an HTTP provider
pointing directly at mistl's `/v1` API continues to work.

The local serve API selects chat providers from its default and model catalogs;
it does not accept a room selector from tc-npc. For multiple rooms, configure
mistl's defaults/catalogs accordingly. Duplicate raw IDs are resolved by mistl.
Its voice endpoints use its TTS/STT configuration; `network-auto` can defer voice
model selection to the remote provider. Set `mist.ai_base_url` when mistl serves
on a different port. `mist.enabled` controls tc-npc's optional integration and
is not required for this HTTP bridge.

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
mistl 側にも対応するルーム・モデルと音声設定を用意してください。
複数ルームの同名モデルの振り分けは mistl の既定と一覧に従います。

## 中文

使用 `default_ref` 和各任务的 `model_ref` 直接选择模型；嵌入模型使用
`memory.embedding_ref`。未设置的引用继承默认值。禁用连接不会改写引用；运行时
会回退到可用的默认模型。连接不存在、模型为空或默认模型不可用时会报错。
请求不发送 temperature。旧预设及其 ID 在首次加载时迁移，之后不再保存。
房间使用 `mist-network://<room>`，通过 `mistl ai serve` 访问。请同时配置
mistl 中对应的房间、模型和语音设置；多个房间中的同名模型由 mistl 的默认配置
和模型列表决定路由。
