// Thin REST client for the tc-npc server (same origin as the WS endpoint —
// see vite.config.ts's dev proxy for the local-dev equivalent).
import type { ScheduledActionEntry } from "./config-types";
import type {
  AffectHistoryDocument,
  CharacterSummary,
  ConfigDocument,
  MemoryDocument,
  PersonDetail,
  PersonRecord,
  VrmModel,
} from "./types";

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, init);
  if (!res.ok) {
    const body = await res.text().catch(() => "");
    throw new Error(`${res.status} ${res.statusText}${body ? `: ${body}` : ""}`);
  }
  if (res.status === 204) return undefined as T;
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

export function getState(): Promise<unknown> {
  return request("/api/state");
}

export function getConfig(): Promise<ConfigDocument> {
  return request("/api/config");
}

export function putConfig(config: ConfigDocument): Promise<void> {
  return request("/api/config", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(config),
  });
}

/**
 * GET /api/audio/devices — the audio endpoints the *server's* host can see
 * (it owns the mic and the speakers; the browser is only a remote control,
 * so this deliberately isn't navigator.mediaDevices).
 *
 * Names feed the 音声 panel's pickers and are saved verbatim into
 * config.speech.{input,output}_device, which the server matches as a
 * case-insensitive substring. `default_*` is what an empty setting resolves
 * to, used only to label the "OS default" entry.
 */
export interface AudioDevices {
  input: string[];
  output: string[];
  default_input: string | null;
  default_output: string | null;
}

export function getAudioDevices(): Promise<AudioDevices> {
  return request("/api/audio/devices");
}

export function getCharacters(): Promise<CharacterSummary[]> {
  return request("/api/characters");
}

/** Body is a tc-town export JSON document, forwarded verbatim. */
export function importCharacter(data: unknown): Promise<void> {
  return request("/api/characters/import", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(data),
  });
}

export function activateCharacter(id: string): Promise<void> {
  return request(`/api/characters/${encodeURIComponent(id)}/activate`, { method: "POST" });
}

/**
 * Point a character at a VRM in the server's model folder, or clear the
 * assignment with `file: null`. The model has to already be in the folder —
 * the server rejects a name that isn't there rather than storing a reference
 * that could never resolve.
 */
export function setCharacterAvatar(id: string, file: string | null): Promise<CharacterSummary> {
  return request(`/api/characters/${encodeURIComponent(id)}/avatar`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ file }),
  });
}

// --- VRM model folder -------------------------------------------------
//
// tc-npc runs on the operator's own machine, so the model library is just
// `{data_dir}/vrm/` — a `.vrm` copied in there by hand shows up in this
// listing with no import step. `addVrmModel` is a convenience over doing
// that copy from the browser, not a separate storage mechanism.

/**
 * Models in the folder, plus its absolute path (shown in the UI so the
 * operator knows where to drop files without going through the browser) and
 * the standalone default avatar's file name (`""` for none).
 */
export function getVrmModels(): Promise<{ models: VrmModel[]; dir: string; default: string }> {
  return request("/api/vrm");
}

/**
 * Set the avatar shown when no active character supplies one, or clear it
 * with `file: null`. This is what makes a model usable on its own: the NPC
 * chats with no character sheet loaded, so an avatar mustn't require
 * importing a tc-town export first.
 */
export function setDefaultAvatar(file: string | null): Promise<{ ok: boolean; file: string }> {
  return request("/api/vrm/default", {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ file }),
  });
}

/** Copy a `.vrm` into the model folder. The body is the raw model bytes;
 *  an existing model of the same name is replaced. */
export function addVrmModel(file: File): Promise<VrmModel> {
  return request(`/api/vrm/${encodeURIComponent(file.name)}`, {
    method: "POST",
    headers: { "Content-Type": "model/gltf-binary" },
    body: file,
  });
}

export function deleteVrmModel(file: string): Promise<void> {
  return request(`/api/vrm/${encodeURIComponent(file)}`, { method: "DELETE" });
}

/** Snapshot of the memory module: the short-term summary plus the
 *  newest-first-able long-term store, for the 感情 tab's memory panel. */
export function getMemory(): Promise<MemoryDocument> {
  return request("/api/memory");
}

/**
 * The server's rolling buffer of recent `affect` snapshots (oldest first),
 * for seeding the 感情 tab's trend sparkline on page load — otherwise that
 * line has nothing to draw until the NPC's next turn produces a live WS
 * frame. `limit` is optional; the server defaults it to (and clamps it at)
 * the buffer's own cap, so omitting it just returns everything currently
 * held.
 */
export function getAffectHistory(limit?: number): Promise<AffectHistoryDocument> {
  return request(limit === undefined ? "/api/affect/history" : `/api/affect/history?limit=${limit}`);
}

/**
 * Fire one scheduled announcement right now, bypassing both the clock and
 * `scheduler.enabled` — the 予定 view's "テスト実行" button, equivalent to
 * the Go agent-scheduler TUI's `[t] Test Playback`.
 *
 * `index` picks the saved announcement; `text` / `chime_file` / `actions`
 * override it with what's currently on screen, so a test doesn't have to wait
 * for the debounced autosave to land. `fired: false` means the entry is a
 * no-op (no text and no actions) and wouldn't produce anything on the real
 * schedule either; `spoke` / `actions` say which half actually ran.
 */
export interface SchedulerTestRequest {
  index?: number;
  text?: string;
  chime_file?: string;
  actions?: ScheduledActionEntry[];
}

export interface SchedulerTestResult {
  ok: boolean;
  fired: boolean;
  spoke: boolean;
  actions: number;
}

export function testAnnouncement(body: SchedulerTestRequest): Promise<SchedulerTestResult> {
  return request("/api/scheduler/test", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

/** List all tracked people, newest-seen first — the 人物 tab's roster. */
export function getPeople(): Promise<{ people: PersonRecord[] }> {
  return request("/api/people");
}

/** One person plus their long-term memories, for the 人物 tab's detail panel. */
export function getPerson(id: string): Promise<PersonDetail> {
  return request(`/api/people/${encodeURIComponent(id)}`);
}

/** Manually add a person (source: "manual") from the 人物 tab. */
export function createPerson(body: { name: string; notes?: string }): Promise<{ person: PersonRecord }> {
  return request("/api/people", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

export function updatePerson(
  id: string,
  body: { name?: string; aliases?: string[]; notes?: string; appearance?: string },
): Promise<{ person: PersonRecord }> {
  return request(`/api/people/${encodeURIComponent(id)}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

export function deletePerson(id: string): Promise<void> {
  return request(`/api/people/${encodeURIComponent(id)}`, { method: "DELETE" });
}

/**
 * Connection probe for the AI settings form: the server builds an
 * OpenAI-compatible client from the given endpoint and lists its models —
 * a successful non-empty listing doubles as the connection test (per the
 * tc-* suite's settings convention; no separate "test" button).
 *
 * `apiKey: "***"` means "use the key already saved in config for `section`",
 * unless `providerId` is given — in that case the server prefers the saved
 * key for that provider id (config.providers[]) instead. `section` stays
 * optional for backward compatibility with pre-provider/preset callers and
 * defaults to `"api"` on the server when omitted.
 */
export interface LlmProbeRequest {
  baseUrl: string;
  apiKey: string;
  section?: "api" | "tts" | "stt";
  providerId?: string;
}

export function listModels(body: LlmProbeRequest): Promise<{ models: string[] }> {
  return request("/api/llm/models", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

export function listVoices(body: LlmProbeRequest): Promise<{ voices: string[] }> {
  return request("/api/llm/voices", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

/**
 * POST /api/llm/ocr — runs the 視覚(vision) task's configured model over one
 * rendered PDF page image and returns the extracted text (see
 * lib/pdf-ocr.ts, ChatView.tsx's file-drop composer entry point).
 *
 * `pageNumber`/`totalPages` are handed to the server rather than baked into
 * a client-side prompt, so the OCR instruction text lives in one place
 * (alongside the rest of the task's prompt-building) instead of being
 * duplicated across the client/server boundary.
 *
 * The call goes through the server rather than straight to the upstream
 * (which is what tc-assistant2 did) because `GET /api/config` masks every
 * `api_key` as `"***"`: the web UI structurally cannot hold a credential, so
 * anything needing inference comes back through here. A vision task with no
 * connection configured answers `412` rather than attempting the call —
 * lib/task-connection.ts gates the UI on the same condition, so that status
 * should not normally be reachable.
 */
export interface OcrPageRequest {
  imageDataUrl: string;
  pageNumber: number;
  totalPages: number;
}

export function ocrPdfPage(body: OcrPageRequest): Promise<{ text: string }> {
  return request("/api/llm/ocr", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
}

/**
 * `POST /api/llm/transcribe/:filename` — runs the 音声認識(stt) task's
 * configured model over one audio clip and returns the transcript (see
 * lib/audio-transcribe.ts, ChatView.tsx's file-drop composer entry point and
 * mic recording). The clip is the raw request body (mirrors
 * `addVrmModel`'s upload shape above) rather than a multipart form or
 * base64 JSON field, so the server can hand the bytes straight to
 * npc_llm::LlmClient::transcribe without decoding anything first;
 * `filename` rides in the path only to preserve the original extension for
 * logging/debugging, the way `addVrmModel` does for `.vrm` uploads.
 *
 * Note that `filename` really is diagnostics-only: the server hands every
 * clip to the upstream as `speech.wav`, which is what the voice cascade has
 * always sent. A `.mp3` still works against a server that sniffs the content
 * rather than trusting the extension; carrying the real extension through
 * would mean widening `LlmClient::transcribe`, which the cascade shares.
 */
export function transcribeAudio(file: Blob, filename: string): Promise<{ text: string }> {
  return request(`/api/llm/transcribe/${encodeURIComponent(filename)}`, {
    method: "POST",
    headers: { "Content-Type": file.type || "application/octet-stream" },
    body: file,
  });
}

/** One persisted chat turn, oldest-first, as returned by the history endpoint. */
export interface ChatHistoryEntry {
  time: string;
  speaker?: string;
  input: string;
  output: string;
}

/** Chat transcript backfill for a fresh page load — useNpcSocket prepends
 *  this to the live `timeline` on mount so a reload doesn't blank the chat
 *  tab. `limit` defaults to 200 turns, matching the server default. */
export function getChatHistory(limit = 200): Promise<{ entries: ChatHistoryEntry[] }> {
  return request(`/api/chat/history?limit=${limit}`);
}
