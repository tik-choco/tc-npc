import { useI18n } from "../hooks/useI18n";
import { aiTranslate } from "../lib/ai-messages";
import { useEffect, useRef, useState } from "preact/hooks";
import { Check, ChevronDown, Copy, Plus } from "lucide-preact";
import { createProvider, deleteProvider, patchProvider, providerKind, roomIdFromBaseUrl,
  networkProviderBaseUrl, type LlmProviderV1 } from "@tik-choco/mistai/llm-config";
import { matchesModelQuery } from "@tik-choco/mistai/preact";
import type { ProviderEntry } from "../lib/config-types";
import { addRoom, fromRef, readProviders, sharedConfig, toRef, writeShared, type Mutate } from "../lib/llm-config";
import type { ConfigDocument } from "../lib/types";
import type { AiTranslate } from "../lib/ai-messages";
import type { CatalogStatus } from "../hooks/useRestModelCatalog";
import { AiPopover } from "./AiPopover";

export function CommitField({ label, value, commit, password = false }: {
  label: string; value: string; commit(value: string): string | void; password?: boolean;
}) {
  const { lang } = useI18n(), t = aiTranslate(lang);
  const [draft, setDraft] = useState(value), [error, setError] = useState(""), [saved, setSaved] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const cancelled = useRef(false);
  useEffect(() => setDraft(value), [value]);
  return <label class="ai-commit-field"><span>{label}</span><input aria-label={label} type={password ? "password" : "text"}
    autoComplete="off" value={draft} aria-invalid={!!error} onInput={event => { setDraft(event.currentTarget.value); setError(""); setSaved(false); }}
    onBlur={event => { if (cancelled.current) { cancelled.current = false; return; } if (event.currentTarget.value !== value) { const result = commit(event.currentTarget.value); setError(result || ""); setSaved(!result); clearTimeout(timer.current); timer.current = setTimeout(() => setSaved(false), 1200); } }}
    onKeyDown={event => { if (event.key === "Enter") event.currentTarget.blur(); if (event.key === "Escape") { event.stopPropagation(); cancelled.current = true; setDraft(value); setError(""); event.currentTarget.blur(); } }} />
    {error ? <small role="status" class="ai-warning">{error}</small> : saved && <small role="status" class="ai-field-saved">{t("field-saved")}</small>}
  </label>;
}
export function AiSwitch({ checked, label, change }: { checked: boolean; label: string; change(): void }) {
  return <button type="button" class="ai-switch" role="switch" aria-checked={checked} aria-label={label} onClick={change}><span /></button>;
}

export function ConnectionCards({ config, mutate, t, status }: { config: ConfigDocument; mutate: Mutate; t: AiTranslate; status(p: ProviderEntry): CatalogStatus | undefined }) {
  return <div class="ai-provider-list">
    <div class="ai-panel-heading"><strong>{t("settings-tab-connection")}</strong><AddConnection config={config} mutate={mutate} t={t} /></div>
    {sharedConfig(config).providers.map(p => <ConnectionCard key={p.id} provider={p} config={config} mutate={mutate} t={t} status={status(readProviders(config).find(row => row.id === p.id)!)} />)}
  </div>;
}
function ConnectionCard({ provider: p, config, mutate, t, status }: { provider: LlmProviderV1; config: ConfigDocument; mutate: Mutate; t: AiTranslate; status?: CatalogStatus }) {
  const [expanded, setExpanded] = useState(false), [details, setDetails] = useState(false), [pendingUrl, setPendingUrl] = useState("");
  const anchor = useRef<HTMLButtonElement>(null), card = useRef<HTMLElement>(null);
  const room = providerKind(p) === "room";
  function update(patch: Partial<LlmProviderV1>) { mutate(draft => { const shared = sharedConfig(draft); patchProvider(shared, p.id, patch); writeShared(draft, shared); }); }
  function commitUrl(value: string) {
    if (room) {
      if (!value.trim()) return t("room-id-invalid");
      const duplicate = sharedConfig(config).providers.find(other => other.id !== p.id && other.baseUrl === networkProviderBaseUrl(value));
      if (duplicate) return t("connection-room-duplicate");
      update({ baseUrl: networkProviderBaseUrl(value) });
    } else {
      try {
        const url = new URL(value); if (!["http:", "https:"].includes(url.protocol)) return t("connection-url-invalid");
        if (p.apiKey === "***" && url.host !== new URL(p.baseUrl).host) {
          setPendingUrl(value); card.current?.querySelector<HTMLInputElement>("input[type=password]")?.focus(); return t("keyRequired");
        }
        update({ baseUrl: value.trim() });
      } catch { return t("connection-url-invalid"); }
    }
  }
  const text = p.enabled === false ? t("models-disabled") : room ? t("models-cache") :
    status?.phase === "fetching" ? t("models-fetching") : status?.phase === "error" ? t("models-fetch-failed") : t("models-ok", { count: p.models?.length ?? 0 });
  return <article ref={card} class={`ai-provider-card ${p.enabled === false ? "ai-disabled" : ""}`} data-provider-id={p.id}>
    <div class="ai-provider-header">
      <AiSwitch checked={p.enabled !== false} label={`${p.label} ${t("models-enabled")}`} change={() => update({ enabled: p.enabled === false })} />
      <button class="ai-provider-summary" type="button" aria-expanded={expanded} onClick={() => setExpanded(!expanded)}>
        <small class="ai-kind">{room ? t("connection-room") : "HTTP"}</small>
        <span class="ai-provider-name"><strong title={p.label}>{p.label}</strong><small title={p.baseUrl}>{room ? roomIdFromBaseUrl(p.baseUrl) : p.baseUrl}</small></span>
        <ChevronDown size={14} class={expanded ? "ai-chevron expanded" : "ai-chevron"} />
      </button>
      <button ref={anchor} type="button" class={`ai-status ${status?.phase ?? "cache"}`} aria-label={text} title={text} onClick={() => setDetails(!details)}><i /><span>{text}</span></button>
      {details && <AiPopover anchor={anchor} close={() => setDetails(false)}><div class="ai-status-detail"><strong>{text}</strong>
        <p>{status?.error ?? (room ? t("bridge") : "")}</p>{p.modelsFetchedAt && <small>{t("connection-updated", { time: p.modelsFetchedAt })}</small>}
      </div></AiPopover>}
    </div>
    <div class={`ai-disclosure ${expanded ? "open" : ""}`} inert={!expanded}><div><div class="ai-provider-body">
      <CommitField label={t("provider-label")} value={p.label} commit={label => update({ label })} />
      <CommitField label={t(room ? "room-id" : "connection-base-url")} value={room ? roomIdFromBaseUrl(p.baseUrl) : p.baseUrl} commit={commitUrl} />
      {!room && <CommitField label={t("connection-api-key")} value={p.apiKey} password commit={apiKey => {
        if (pendingUrl && apiKey === "***") return t("keyRequired");
        update({ apiKey, ...(pendingUrl ? { baseUrl: pendingUrl } : {}) }); setPendingUrl("");
      }} />}
      <button type="button" class="btn btn-ghost ai-danger" onClick={() => { if (confirm(t("provider-delete-confirm"))) mutate(draft => { const shared = sharedConfig(draft); deleteProvider(shared, p.id); writeShared(draft, shared); }); }}>{t("provider-delete")}</button>
    </div></div></div>
  </article>;
}
let lastKind = "http";
export function AddConnection({ config, mutate, t, roomOnly = false }: { config: ConfigDocument; mutate: Mutate; t: AiTranslate; roomOnly?: boolean }) {
  const [open, setOpen] = useState(false), [kind, setKind] = useState(roomOnly ? "room" : lastKind);
  const [label, setLabel] = useState(""), [url, setUrl] = useState(""), [roomId, setRoomId] = useState(""), [key, setKey] = useState(""), [error, setError] = useState("");
  const anchor = useRef<HTMLButtonElement>(null);
  const rooms = readProviders(config).filter(p => (p.base_url ?? "").startsWith("mist-network://") && p.enabled !== false && !p.provide);
  function create() {
    if (kind === "room") {
      if (!roomId.trim()) { setError(t("connection-room-required")); return; }
      const duplicate = readProviders(config).find(p => p.base_url === networkProviderBaseUrl(roomId));
      if (duplicate && !roomOnly) { setError(t("connection-room-duplicate")); return; }
      mutate(draft => { addRoom(draft, roomId.trim(), label.trim() || roomId.trim(), roomOnly); });
    } else {
      try { if (!["http:", "https:"].includes(new URL(url).protocol)) throw new Error(); } catch { setError(t("connection-url-invalid")); return; }
      mutate(draft => { const shared = sharedConfig(draft); const id = createProvider(shared, label.trim() || new URL(url).host); patchProvider(shared, id, { baseUrl: url.trim(), apiKey: key }); writeShared(draft, shared); });
    }
    setOpen(false); setLabel(""); setUrl(""); setRoomId(""); setKey(""); setError("");
  }
  return <>
    <button ref={anchor} type="button" class="btn btn-ghost" onClick={() => setOpen(!open)}><Plus size={14} />{t(roomOnly ? "room-add" : "provider-add")}</button>
    {open && <AiPopover anchor={anchor} close={() => setOpen(false)}><form class="ai-add-form" onSubmit={event => { event.preventDefault(); create(); }}>
      {!roomOnly && <div class="ai-tabs" role="tablist">{["http", "room"].map(id => <button type="button" role="tab" aria-selected={kind === id} onClick={() => { setKind(id); lastKind = id; setError(""); }}>{id === "http" ? "HTTP" : t("connection-room")}</button>)}</div>}
      {roomOnly && rooms.length > 0 && <label>{t("room-from-connections")}<select value="" onChange={event => { const id = event.currentTarget.value; mutate(draft => { const room = readProviders(draft).find(p => p.id === id); if (room) room.provide = true; }); setOpen(false); }}>
        <option value="">{t("room-select")}</option>{rooms.map(p => <option value={p.id}>{p.label || p.id}</option>)}
      </select></label>}
      <label>{t("connection-name")}<input value={label} onInput={event => setLabel(event.currentTarget.value)} /></label>
      {kind === "http" ? <><label>{t("connection-base-url")}<input value={url} onInput={event => setUrl(event.currentTarget.value)} /></label><label>{t("connection-api-key")}<input type="password" value={key} onInput={event => setKey(event.currentTarget.value)} /></label></> :
        <label>{t("room-id")}<input value={roomId} onInput={event => setRoomId(event.currentTarget.value)} /><button type="button" class="btn btn-ghost" onClick={() => setRoomId(crypto.randomUUID().replaceAll("-", "").slice(0, 20))}>{t("room-random")}</button></label>}
      {error && <p class="ai-warning" role="alert">{error}</p>}
      <div class="ai-form-actions"><button type="button" class="btn btn-ghost" onClick={() => setOpen(false)}>{t("provider-cancel")}</button><button type="submit" class="btn btn-primary">{t("connection-submit")}</button></div>
    </form></AiPopover>}
  </>;
}
function RoomChip({ room, mutate, t }: { room: ProviderEntry; mutate: Mutate; t: AiTranslate }) {
  const [copied, setCopied] = useState(false), [error, setError] = useState("");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  return <span class={`ai-room-chip ${room.provide ? "selected" : ""}`}>
    <button type="button" aria-pressed={room.provide ?? false} title={t("bridge")} onClick={() => mutate(draft => { const p = readProviders(draft).find(p => p.id === room.id); if (p) p.provide = !p.provide; })}>
      {room.label || room.id}<small>{t(room.provide ? "models-cache" : "sharing-stopped")}</small>
    </button>
    <button type="button" class="ai-copy" title={error || t(copied ? "room-copied" : "room-copy-id")} aria-label={t("room-copy-id")} onClick={async () => {
      try { await navigator.clipboard.writeText(roomIdFromBaseUrl(room.base_url ?? "")); setCopied(true); clearTimeout(timer.current); timer.current = setTimeout(() => setCopied(false), 1200); } catch { setError(t("room-copy-failed")); }
    }}>{copied ? <Check size={14} /> : <Copy size={14} />}</button>
  </span>;
}
export function SharingSettings({ config, mutate, t }: { config: ConfigDocument; mutate: Mutate; t: AiTranslate }) {
  const [query, setQuery] = useState("");
  const shared = sharedConfig(config), rooms = readProviders(config).filter(p => p.enabled !== false && (p.base_url ?? "").startsWith("mist-network://"));
  const http = shared.providers.filter(p => p.enabled !== false && providerKind(p) === "http");
  function has(room: ProviderEntry, ref: { providerId: string; model: string }) { return room.shared?.some(r => r.provider_id === ref.providerId && r.model === ref.model) ?? false; }
  return <div class="ai-sharing">
    <p class="field-hint">{t("bridge")}</p><div class="ai-room-chips">{rooms.map(room => <RoomChip key={room.id} room={room} mutate={mutate} t={t} />)}<AddConnection config={config} mutate={mutate} t={t} roomOnly /></div>
    {!rooms.length && <p class="field-hint">{t("sharing-no-rooms")}</p>}
    {!http.length && <p class="field-hint">{t("sharing-no-http")}</p>}
    <input type="search" aria-label={t("models-search")} placeholder={t("models-search")} value={query} onInput={event => setQuery(event.currentTarget.value)} />
    {http.map(p => {
      const refs = [...new Set([...(p.models ?? []), ...rooms.flatMap(room => (room.shared ?? []).filter(ref => ref.provider_id === p.id).map(ref => ref.model))])]
        .map(model => ({ providerId: p.id, model })).filter(ref => matchesModelQuery(ref, p, query))
        .sort((a, b) => Number(rooms.some(room => has(room, b))) - Number(rooms.some(room => has(room, a))) || a.model.localeCompare(b.model));
      return <section key={p.id} class="ai-share-group"><h4>{p.label} <small>HTTP</small></h4>{refs.map(ref => <div class="ai-share-row" key={ref.model}><span>{ref.model}</span><div>
        {rooms.map(room => <button type="button" class={has(room, ref) ? "selected" : ""} aria-pressed={has(room, ref)} onClick={() => mutate(draft => {
          const r = readProviders(draft).find(p => p.id === room.id); if (!r) return;
          r.shared = has(r, ref) ? (r.shared ?? []).filter(item => !sameRef(item, ref)) : [...r.shared ?? [], toRef(ref)!];
        })}>{room.label || room.id}{has(room, ref) && <Check size={12} />}</button>)}
      </div></div>)}</section>;
    })}
  </div>;
}
function sameRef(a: unknown, b: { providerId: string; model: string }) { const ref = fromRef(a); return ref?.providerId === b.providerId && ref.model === b.model; }