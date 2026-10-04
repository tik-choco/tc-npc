import { useEffect, useRef, useState } from "preact/hooks";
import { aiTranslate } from "../lib/ai-messages";
import { useI18n } from "../hooks/useI18n";
import { useRestModelCatalog } from "../hooks/useRestModelCatalog";
import { useMistStatus } from "../hooks/useMistStatus";
import { liveRoom } from "../lib/mist-status";
import { LLM_TASKS, setTaskRef, sharedConfig, taskRef, toRef, type Mutate } from "../lib/llm-config";
import type { ConfigDocument } from "../lib/types";
import type { ModelRefV1 } from "@tik-choco/mistai/llm-config";
import { resolveModel, providerKind, isModelRef } from "@tik-choco/mistai/llm-config";
import { listVoices } from "../lib/api";
import { RefPicker, EffortPicker } from "./RefPicker";
import { ConnectionCards, SharingSettings, CommitField } from "./AiConnections";
import "@tik-choco/mistai/ui.css";
import "../styles/ai-settings.css";

export function AiSettings({ config, mutate }: { config: ConfigDocument; mutate: Mutate }) {
  const { lang, t: appT } = useI18n(), t = aiTranslate(lang);
  const [tab, setTab] = useState<"connection" | "tasks" | "sharing">("connection");
  const [recent, setRecent] = useState<ModelRefV1[]>(() => {
    try { const stored: unknown = JSON.parse(sessionStorage.getItem("tc-npc:recent-models") ?? "[]"); return Array.isArray(stored) ? stored.filter(isModelRef).slice(0, 8) : []; } catch { return []; }
  });
  useEffect(() => { try { sessionStorage.setItem("tc-npc:recent-models", JSON.stringify(recent)); } catch { /* UI history is optional. */ } }, [recent]);
  const catalog = useRestModelCatalog(config, mutate);
  const live = useMistStatus();
  const panel = useRef<HTMLDivElement>(null), from = useRef<number | null>(null), animation = useRef<Animation | null>(null);
  useEffect(() => {
    if (from.current !== null && panel.current && !matchMedia("(prefers-reduced-motion: reduce)").matches) {
      animation.current?.cancel();
      animation.current = panel.current.animate([{ height: `${from.current}px` }, { height: `${panel.current.getBoundingClientRect().height}px` }], { duration: 200, easing: "cubic-bezier(.2,.8,.2,1)" });
    }
    from.current = null;
    return () => animation.current?.cancel();
  }, [tab]);
  const shared = sharedConfig(config);
  shared.providers = shared.providers.map(p => {
    const room = liveRoom(live, { base_url: p.baseUrl });
    return room?.joined ? { ...p, models: room.models } : p;
  });
  function remember(ref?: ModelRefV1) { if (ref) setRecent(current => [ref, ...current.filter(r => r.providerId !== ref.providerId || r.model !== ref.model)].slice(0, 8)); }
  const picker = { providers: shared.providers, recent, refresh: catalog.revalidate, t };
  const taskLabel = (id: string) => t(`task${id[0]!.toUpperCase()}${id.slice(1)}` as "taskTalk");
  function patchSection(key: string, patch: Record<string, unknown>) { mutate(draft => { draft[key] = { ...(draft[key] as object ?? {}), ...patch }; }); }
  return <section class="npc-ai" aria-label={appT("settings.connection.title")}>
    <div class="ai-tabs" role="tablist"><span class="ai-tab-highlight" style={{ transform: `translateX(${["connection", "tasks", "sharing"].indexOf(tab) * 100}%)` }} />
      {(["connection", "tasks", "sharing"] as const).map(id => <button type="button" role="tab" aria-selected={tab === id} onClick={() => { if (tab !== id) { from.current = panel.current?.getBoundingClientRect().height ?? null; setTab(id); catalog.revalidate(); } }}>{t(`settings-tab-${id}`)}</button>)}
    </div>
    <div ref={panel} role="tabpanel" class="ai-tab-panel">
      {(live.sync?.error || live.error) && <p class="ai-warning" role="alert">{t("syncError", { error: live.sync?.error || live.error || "" })}</p>}
      {live.sync?.pending && <p role="status" class="field-hint">{t("syncPending")}</p>}
      {live.sync?.warnings.map(warning => <p class="ai-warning" role="status">{t("syncWarning", { warning })}</p>)}
      {tab === "connection" && <ConnectionCards config={config} mutate={mutate} t={t} status={catalog.status} live={live} />}
      {tab === "sharing" && <SharingSettings config={config} mutate={mutate} t={t} live={live} />}
      {tab === "tasks" && <div class="ai-task-list">
        <div class="ai-task-row"><span>{t("models-default")}</span><RefPicker {...picker} label={t("models-default")} value={shared.defaultModel} clear={false} onChange={ref => { mutate(draft => { draft.default_ref = toRef(ref); }); remember(ref); }} /></div>
        {LLM_TASKS.map(task => {
          const section = (config[task.section] ?? {}) as Record<string, unknown>;
          const label = taskLabel(task.id), ref = taskRef(config, task.id);
          const reasoning = !["embedding", "tts", "stt"].includes(task.id);
          const effort = typeof section.reasoning_effort === "string" && section.reasoning_effort ? section.reasoning_effort :
            (config.api as { reasoning_effort?: string })?.reasoning_effort || "none";
          return <div key={task.id} class="ai-task-block">
            <div class={`ai-task-row ${reasoning ? "with-effort" : ""}`}><span title={appT(task.tipKey)}>{label}</span>
              <RefPicker {...picker} label={label} value={ref} inherited={shared.defaultModel} voice={task.id === "tts" || task.id === "stt"} onChange={next => { mutate(draft => setTaskRef(draft, task.id, next)); remember(next); }} />
              {reasoning && <EffortPicker label={`${label} ${t("models-effort")}`} value={effort} t={t} onChange={value => patchSection(task.section, { reasoning_effort: value })} />}
            </div>
            {task.id === "tts" && <div class="ai-task-extras"><VoiceField config={config} mutate={mutate} label={appT("task.tts.voice")} />
              <NumberField label={appT("task.tts.speed")} value={Number(section.speed ?? 1)} step={0.1} commit={value => patchSection("tts", { speed: value })} />
              <NumberField label={appT("task.tts.maxLen")} value={Number(section.max_len ?? 200)} commit={value => patchSection("tts", { max_len: value })} />
            </div>}
            {task.id === "stt" && <div class="ai-task-extras">
              <NumberField label={appT("task.stt.silence")} value={Number(section.silence_duration ?? 1.5)} step={0.1} commit={value => patchSection("stt", { silence_duration: value })} />
              <NumberField label={appT("task.stt.threshold")} value={Number(section.input_threshold ?? 0.01)} step={0.01} commit={value => patchSection("stt", { input_threshold: value })} />
            </div>}
          </div>;
        })}
      </div>}
    </div>
  </section>;
}
function NumberField({ label, value, step = 1, commit }: { label: string; value: number; step?: number; commit(value: number): void }) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  return <label>{label}<input aria-label={label} type="number" step={step} value={draft} onInput={event => setDraft(event.currentTarget.value)} onBlur={() => {
    const parsed = Number(draft); if (draft.trim() && Number.isFinite(parsed)) commit(parsed); else setDraft(String(value));
  }} onKeyDown={event => { if (event.key === "Enter") event.currentTarget.blur(); if (event.key === "Escape") { setDraft(String(value)); event.currentTarget.blur(); } }} /></label>;
}
function VoiceField({ config, mutate, label }: { config: ConfigDocument; mutate: Mutate; label: string }) {
  const [voices, setVoices] = useState<string[]>([]);
  const shared = sharedConfig(config), target = resolveModel(shared, taskRef(config, "tts"));
  const provider = target && shared.providers.find(p => p.id === target.providerId);
  const tts = (config.tts ?? {}) as { voice?: string };
  useEffect(() => {
    setVoices([]); if (!provider || providerKind(provider) === "room") return;
    let cancelled = false;
    void listVoices({ baseUrl: provider.baseUrl, apiKey: provider.apiKey, providerId: provider.id, section: "tts" }).then(result => { if (!cancelled) setVoices(result.voices); }).catch(() => {});
    return () => { cancelled = true; };
  }, [provider?.baseUrl, provider?.apiKey, provider?.id]);
  // Custom voice names remain valid for endpoints without a voice-list API.
  return <div><CommitField label={label} value={tts.voice ?? ""} commit={voice => mutate(draft => { draft.tts = { ...(draft.tts as object ?? {}), voice }; })} />
    {voices.length > 0 && <select aria-label={label} value={tts.voice ?? ""} onChange={event => { const voice = event.currentTarget.value; mutate(draft => { draft.tts = { ...(draft.tts as object ?? {}), voice }; }); }}><option value="" />{[...new Set([...(tts.voice ? [tts.voice] : []), ...voices])].map(voice => <option value={voice}>{voice}</option>)}</select>}
  </div>;
}
