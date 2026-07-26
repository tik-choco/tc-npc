// Thin REST client for the tc-npc server (same origin as the WS endpoint —
// see vite.config.ts's dev proxy for the local-dev equivalent).
import type { ScheduledActionEntry } from "./config-types";
import type { CharacterSummary, ConfigDocument, MemoryDocument, PersonDetail, PersonRecord } from "./types";

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

/** Snapshot of the memory module: the short-term summary plus the
 *  newest-first-able long-term store, for the 感情 tab's memory panel. */
export function getMemory(): Promise<MemoryDocument> {
  return request("/api/memory");
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
 * `apiKey: "***"` means "use the key already saved in config for `section`".
 */
export interface LlmProbeRequest {
  baseUrl: string;
  apiKey: string;
  section: "api" | "tts" | "stt";
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
