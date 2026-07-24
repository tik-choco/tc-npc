// Thin REST client for the tc-npc server (same origin as the WS endpoint —
// see vite.config.ts's dev proxy for the local-dev equivalent).
import type { CharacterSummary, ConfigDocument } from "./types";

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
