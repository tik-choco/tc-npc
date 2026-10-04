import { useRef, useState } from "preact/hooks";
import { Check, ChevronDown } from "lucide-preact";
import { matchesModelQuery, modelKey, sameModel, REASONING_EFFORT_OPTIONS } from "@tik-choco/mistai/preact";
import { NETWORK_VOICE_AUTO_MODEL, providerKind, type LlmProviderV1, type ModelRefV1 } from "@tik-choco/mistai/llm-config";
import type { AiTranslate } from "../lib/ai-messages";
import { AiPopover } from "./AiPopover";

export function RefPicker({ providers, value, inherited, recent, label, onChange, refresh, t, voice = false, clear = true }: {
  providers: LlmProviderV1[]; value?: ModelRefV1; inherited?: ModelRefV1; recent: ModelRefV1[];
  label: string; onChange(ref?: ModelRefV1): void; refresh(): void; t: AiTranslate; voice?: boolean; clear?: boolean;
}) {
  const [open, setOpen] = useState(false), [query, setQuery] = useState("");
  const [source, setSource] = useState(""), [narrow, setNarrow] = useState("");
  const anchor = useRef<HTMLButtonElement>(null);
  const enabled = providers.filter(p => p.enabled !== false);
  const assigned = value ?? inherited;
  const assignedProvider = providers.find(p => p.id === assigned?.providerId);
  const unique = (refs: ModelRefV1[]) => [...new Map(refs.map(ref => [modelKey(ref), ref])).values()];
  const recents = unique([...(assigned ? [assigned] : []), ...recent]).filter(ref =>
    enabled.some(p => p.id === ref.providerId)).slice(0, 8);
  const modelsFor = (p: LlmProviderV1) => unique([
    ...(voice && providerKind(p) === "room" ? [{ providerId: p.id, model: NETWORK_VOICE_AUTO_MODEL }] : []),
    ...(assigned?.providerId === p.id ? [assigned] : []),
    ...(p.models ?? []).map(model => ({ providerId: p.id, model })),
  ]);
  const search = query.trim().length > 0;
  const entries = search ? enabled.filter(p => !narrow || p.id === narrow).flatMap(p =>
    modelsFor(p).filter(ref => matchesModelQuery(ref, p, query))) :
    source === "recent" ? recents : enabled.filter(p => p.id === source).flatMap(modelsFor);
  function choose(ref?: ModelRefV1) { onChange(ref); setOpen(false); }
  const modelLabel = (ref: ModelRefV1) => ref.model === NETWORK_VOICE_AUTO_MODEL ?
    t("models-auto", { provider: providers.find(p => p.id === ref.providerId)?.label ?? "" }) : ref.model;
  function move(event: KeyboardEvent) {
    const root = (event.currentTarget as HTMLElement), target = event.target as HTMLElement;
    const pane = target.closest("[data-pane]")?.getAttribute("data-pane");
    if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault(); root.querySelector<HTMLElement>(`[data-pane='${event.key === "ArrowLeft" ? "sources" : "models"}'] button`)?.focus(); return;
    }
    if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
      event.preventDefault();
      const buttons = [...root.querySelectorAll<HTMLButtonElement>(`[data-pane='${pane ?? "models"}'] button`)];
      const index = buttons.indexOf(target as HTMLButtonElement);
      const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 :
        Math.max(0, Math.min(buttons.length - 1, index + (event.key === "ArrowUp" ? -1 : 1)));
      buttons[next]?.focus();
    }
  }
  return <div class="ai-picker-field">
    <button ref={anchor} type="button" class="ai-picker-trigger" aria-label={label} aria-haspopup="dialog" aria-expanded={open}
      onClick={() => { if (!open) { setQuery(""); setNarrow(""); setSource(enabled.some(p => p.id === assigned?.providerId) ? assigned!.providerId : recents.length ? "recent" : enabled[0]?.id ?? ""); refresh(); } setOpen(!open); }}>
      <span>{assigned ? `${modelLabel(assigned)} · ${assignedProvider?.label ?? assigned.providerId}` : t("models-follow")}</span><ChevronDown size={14} />
    </button>
    {value && (!assignedProvider || assignedProvider.enabled === false) && <small class="ai-warning">{t(assignedProvider ? "models-disabled" : "models-missing")}</small>}
    {open && <AiPopover anchor={anchor} close={() => setOpen(false)} wide>
      <div class="ai-model-picker" role="dialog" aria-label={label} onKeyDown={move}>
        <input type="search" aria-label={t("models-search")} placeholder={t("models-search")} value={query}
          onInput={event => { setQuery(event.currentTarget.value); setNarrow(""); }} />
        <div class="ai-picker-panes">
          <div class="ai-picker-sources" data-pane="sources" role="listbox" aria-label={t("models-sources")}>
            {clear && <button type="button" role="option" aria-selected={!value} onClick={() => choose()}>{t("serverDefault")}</button>}
            {search ? <button type="button" role="option" aria-selected={!narrow} onClick={() => setNarrow("")}>{t("models-all")}</button> :
              recents.length > 0 && <button type="button" role="option" aria-selected={source === "recent"} onClick={() => setSource("recent")}>{t("models-recent")}</button>}
            {enabled.map(p => <button type="button" role="option" key={p.id} title={p.label} aria-selected={search ? narrow === p.id : source === p.id}
              class={search && !modelsFor(p).some(ref => matchesModelQuery(ref, p, query)) ? "ai-dim" : ""}
              onClick={() => search ? setNarrow(p.id) : setSource(p.id)}>
              <strong>{p.label}</strong><small>{providerKind(p) === "room" ? t("connection-room") : "HTTP"} · {search ? modelsFor(p).filter(ref => matchesModelQuery(ref, p, query)).length : p.models?.length ?? 0}</small>
            </button>)}
          </div>
          <div class="ai-picker-models" data-pane="models" role="listbox" aria-label={t("models-list")}>
            {entries.length === 0 && <p class="field-hint">{t("models-empty")}</p>}
            {entries.map((ref, index) => {
              const p = enabled.find(p => p.id === ref.providerId)!;
              const mixed = !search && source === "recent";
              const header = !mixed && (index === 0 || entries[index - 1]?.providerId !== p.id);
              return <div key={modelKey(ref)}>
                {header && <div class="ai-model-group">{p.label} <small>{providerKind(p) === "room" ? t("connection-room") : "HTTP"}</small></div>}
                <button type="button" role="option" aria-selected={sameModel(assigned, ref)} class={providerKind(p) === "room" ? "ai-cached" : ""}
                  ref={element => { if (element && sameModel(assigned, ref) && !search) requestAnimationFrame(() => element.scrollIntoView({ block: "nearest" })); }}
                  onClick={() => choose(ref)}>
                  <span>{modelLabel(ref)}{mixed && <small>{p.label}</small>}</span>{sameModel(assigned, ref) && <Check size={14} />}
                </button>
              </div>;
            })}
          </div>
        </div>
      </div>
    </AiPopover>}
  </div>;
}

export function EffortPicker({ value, onChange, t, label }: { value: string; onChange(value: string): void; t: AiTranslate; label: string }) {
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const level = REASONING_EFFORT_OPTIONS.indexOf(value as typeof REASONING_EFFORT_OPTIONS[number]);
  const bars = (n: number) => <span class="ai-effort-bars" aria-hidden="true">{Array.from({ length: 6 }, (_, i) => <i class={i < n ? "filled" : ""} />)}</span>;
  return <div class="ai-effort">
    <button ref={anchor} type="button" class="ai-picker-trigger" aria-label={label} title={label} aria-expanded={open} onClick={() => setOpen(!open)}>
      {bars(Math.max(0, level))}<span>{t("models-reasoning")} · {t(`effort-${REASONING_EFFORT_OPTIONS[Math.max(0, level)]}`)}</span><ChevronDown size={14} />
    </button>
    {open && <AiPopover anchor={anchor} close={() => setOpen(false)}><div role="listbox" aria-label={label} class="ai-effort-menu"
      onKeyDown={event => {
        const buttons = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("button")];
        const index = buttons.indexOf(event.target as HTMLButtonElement);
        if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
          event.preventDefault(); buttons[event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1 :
            (index + (event.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length]?.focus();
        }
      }}>
      {REASONING_EFFORT_OPTIONS.map((effort, i) => <button type="button" role="option" aria-selected={value === effort}
        onClick={() => { onChange(effort); setOpen(false); }}>{bars(i)}{t(`effort-${effort}`)}{value === effort && <Check size={14} />}</button>)}
    </div></AiPopover>}
  </div>;
}