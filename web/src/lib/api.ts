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
