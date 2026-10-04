import { useEffect, useRef, useState } from "preact/hooks";
import { listModels } from "../lib/api";
import { readProviders, type Mutate } from "../lib/llm-config";
import type { ProviderEntry } from "../lib/config-types";
import type { ConfigDocument } from "../lib/types";
import { isNetworkProviderBaseUrl } from "@tik-choco/mistai/llm-config";

export type CatalogStatus = { phase: "fetching" | "ok" | "error"; error?: string; connection: string };
const pending = new Map<string, { connection: string; promise: Promise<void> }>();
const statuses = new Map<string, CatalogStatus>();
const listeners = new Set<() => void>();
const connection = (p: ProviderEntry) => JSON.stringify([p.base_url, p.api_key, p.enabled !== false]);
const notify = () => listeners.forEach(cb => cb());

// A REST catalog is necessary because credentials are masked in the browser.
// Room caches are owned by mistl; this API has no per-room discovery endpoint.
export function useRestModelCatalog(config: ConfigDocument, mutate: Mutate) {
  const current = useRef({ config, mutate });
  current.current = { config, mutate };
  const [, render] = useState(0);
  useEffect(() => { const cb = () => render(n => n + 1); listeners.add(cb); return () => { listeners.delete(cb); }; }, []);
  async function refresh(id: string, force = false): Promise<void> {
    const provider = readProviders(current.current.config).find(p => p.id === id);
    if (!provider || provider.enabled === false || isNetworkProviderBaseUrl(provider.base_url ?? "")) return;
    const key = connection(provider), existing = pending.get(id);
    if (existing) return existing.connection === key ? existing.promise : existing.promise.then(() => refresh(id, true));
    if (!force && Date.now() - Date.parse(provider.models_fetched_at ?? "") < 10_000) return;
    const work = Promise.resolve().then(async () => {
      statuses.set(id, { phase: "fetching", connection: key }); notify();
      try {
        const result = await listModels({ providerId: id, baseUrl: provider.base_url ?? "", apiKey: provider.api_key ?? "" });
        const live = readProviders(current.current.config).find(p => p.id === id);
        if (!live || connection(live) !== key || live.enabled === false) return;
        const models = [...new Set(result.models)].sort((a, b) => a.localeCompare(b));
        current.current.mutate(draft => {
          const p = readProviders(draft).find(p => p.id === id);
          if (p && connection(p) === key) { p.models = models; p.models_fetched_at = new Date().toISOString(); }
        });
        statuses.set(id, { phase: "ok", connection: key });
      } catch (error) {
        const live = readProviders(current.current.config).find(p => p.id === id);
        if (live && connection(live) === key)
          statuses.set(id, { phase: "error", connection: key, error: error instanceof Error ? error.message : String(error) });
      } finally { pending.delete(id); notify(); }
    });
    pending.set(id, { connection: key, promise: work });
    return work;
  }
  function revalidate() { readProviders(current.current.config).forEach(p => { void refresh(p.id); }); }
  const keys = readProviders(config).map(p => `${p.id}:${connection(p)}`).join("|");
  const previous = useRef(new Map<string, string>());
  useEffect(() => {
    for (const provider of readProviders(current.current.config)) {
      const key = connection(provider), old = previous.current.get(provider.id);
      void refresh(provider.id, old !== undefined && old !== key);
      previous.current.set(provider.id, key);
    }
  }, [keys]);
  return { revalidate, status: (p: ProviderEntry) => {
    const state = statuses.get(p.id);
    return state?.connection === connection(p) ? state : undefined;
  } };
}