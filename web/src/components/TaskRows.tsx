// タスク別モデル割り当て行(設定 > タスク タブ)。
//
// tc-docs/drafts/llm-settings-common-v1.md §3.2 に準拠: 全行を同じ2カラム
// (ラベル + フィールド群)スタイルに揺れなく統一し、常時表示のヒント段落は
// 置かない。行ごとの説明文はラベルの data-tip 属性が持つ純CSSツールチップ
// (settings-llm.css の `span[data-tip]:hover::after`)に一本化していて、
// ラベルは1語に保つ(タスク名を見て意味が分かるので、常時表示の説明文で場所を
// 取らない)。tc-translate の SettingsModal.tsx タスクタブ / VoiceTaskRows と
// 同じ .task-model-item / .task-model-fields / .task-model-field 構造を移植。
//
// 行の並びは LLM_TASKS (lib/llm-config.ts) の順で固定。先頭の「既定」だけは
// LLM_TASKS に含まれない特別行で、default_preset_id と config.api の推論
// エフォートをここで編集する。
import { useState } from "preact/hooks";
import {
  LLM_TASKS,
  presetLabel,
  providerOf,
  readDefaultPresetId,
  readPresets,
  resolveTaskPreset,
  setDefaultPreset,
  setTaskPreset,
  taskPresetId,
  type LlmTaskId,
  type Mutate,
} from "../lib/llm-config";
import { ModelPicker, ReasoningEffortField, commitOnEnter } from "./SettingsFields";
import type { Translate } from "../lib/i18n";
import type { ApiSection, PresetEntry, SpeechEndpointSection } from "../lib/config-types";
import type { ConfigDocument } from "../lib/types";
import "../styles/settings-llm.css";
import "../styles/components.css";

function readApi(config: ConfigDocument): ApiSection {
  return (config.api as ApiSection | undefined) ?? {};
}

function readSpeechSection(config: ConfigDocument, key: "tts" | "stt"): SpeechEndpointSection {
  return (config[key] as SpeechEndpointSection | undefined) ?? {};
}

/** Reads a numeric field off a speech section (speed/max_len/silence_duration/
 * input_threshold aren't in SpeechEndpointSection's named props, only its
 * index signature), falling back to `fallback` when absent or not a number. */
function readNumber(section: SpeechEndpointSection, field: string, fallback: number): number {
  const value = section[field];
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

/** Blur-commit number input, mirroring SettingsFields.TextField's draft/commit
 * split but for numeric fields (task.tts.speed etc.) that TextField doesn't
 * cover. Invalid input (empty, NaN) is dropped silently on blur rather than
 * written back, and the draft snaps back to the last committed value. */
function NumberField(props: { value: number; step?: number; onCommit: (value: number) => void }) {
  const { value, step, onCommit } = props;
  const [draft, setDraft] = useState(String(value));

  return (
    <input
      type="number"
      step={step}
      value={draft}
      onInput={(e) => setDraft((e.target as HTMLInputElement).value)}
      onFocus={() => setDraft(String(value))}
      onBlur={() => {
        const parsed = Number(draft);
        if (Number.isFinite(parsed) && parsed !== value) {
          onCommit(parsed);
        } else {
          setDraft(String(value));
        }
      }}
      onKeyDown={commitOnEnter}
    />
  );
}

/** The 既定 row: default_preset_id + config.api.reasoning_effort. Not part of
 * LLM_TASKS (it isn't a task, it's what tasks fall back to), so it's rendered
 * by hand ahead of the LLM_TASKS.map below. */
function DefaultRow(props: { t: Translate; config: ConfigDocument; mutate: Mutate; presets: PresetEntry[] }) {
  const { t, config, mutate, presets } = props;
  const defaultPresetId = readDefaultPresetId(config);
  const api = readApi(config);

  return (
    <div class="task-model-item">
      <span data-tip={t("task.default.tip")}>{t("task.default")}</span>
      <div class="task-model-fields">
        <div class="task-model-field">
          <select
            value={defaultPresetId}
            onChange={(e) => setDefaultPreset(mutate, (e.target as HTMLSelectElement).value)}
          >
            <option value="">{t("task.unset")}</option>
            {presets.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {presetLabel(preset)}
              </option>
            ))}
          </select>
        </div>
        <div class="task-model-field">
          <ReasoningEffortField
            t={t}
            value={api.reasoning_effort ?? "none"}
            onChange={(v) =>
              mutate((draft) => {
                const current = (draft.api as ApiSection | undefined) ?? {};
                draft.api = { ...current, reasoning_effort: v };
              })
            }
            name="task-default-effort"
          />
        </div>
      </div>
    </div>
  );
}

/** Extra fields for the tts row: voice picker, speed, max length. Written
 * back onto config.tts (spread-then-patch, same convention as
 * lib/llm-config.ts's writers). */
function TtsExtraFields(props: { t: Translate; config: ConfigDocument; mutate: Mutate }) {
  const { t, config, mutate } = props;
  const tts = readSpeechSection(config, "tts");
  const api = readApi(config);
  const preset = resolveTaskPreset(config, "tts");
  const provider = providerOf(config, preset);
  const baseUrl = provider ? provider.base_url ?? "" : tts.base_url || api.base_url || "";
  const apiKey = provider ? provider.api_key ?? "" : tts.api_key || api.api_key || "";

  function patchTts(patch: Partial<SpeechEndpointSection>): void {
    mutate((draft) => {
      const current = (draft.tts as SpeechEndpointSection | undefined) ?? {};
      draft.tts = { ...current, ...patch };
    });
  }

  return (
    <>
      <div class="task-model-field">
        <span>{t("task.tts.voice")}</span>
        <ModelPicker
          t={t}
          value={tts.voice ?? ""}
          placeholder={t("task.tts.voice")}
          baseUrl={baseUrl}
          apiKey={apiKey}
          section="tts"
          kind="voices"
          itemLabel={t("picker.item.voice")}
          providerId={provider?.id}
          onChange={(v) => patchTts({ voice: v })}
        />
      </div>
      <div class="task-model-field">
        <span>{t("task.tts.speed")}</span>
        <NumberField
          value={readNumber(tts, "speed", 1)}
          step={0.1}
          onCommit={(v) => patchTts({ speed: v })}
        />
      </div>
      <div class="task-model-field">
        <span data-tip={t("task.tts.maxLen.tip")}>{t("task.tts.maxLen")}</span>
        <NumberField
          value={readNumber(tts, "max_len", 200)}
          step={1}
          onCommit={(v) => patchTts({ max_len: v })}
        />
      </div>
    </>
  );
}

/** Extra fields for the stt row: silence cutoff, input threshold. */
function SttExtraFields(props: { t: Translate; config: ConfigDocument; mutate: Mutate }) {
  const { t, config, mutate } = props;
  const stt = readSpeechSection(config, "stt");

  function patchStt(patch: Partial<SpeechEndpointSection>): void {
    mutate((draft) => {
      const current = (draft.stt as SpeechEndpointSection | undefined) ?? {};
      draft.stt = { ...current, ...patch };
    });
  }

  return (
    <>
      <div class="task-model-field">
        <span data-tip={t("task.stt.silence.tip")}>{t("task.stt.silence")}</span>
        <NumberField
          value={readNumber(stt, "silence_duration", 1.5)}
          step={0.1}
          onCommit={(v) => patchStt({ silence_duration: v })}
        />
      </div>
      <div class="task-model-field">
        <span data-tip={t("task.stt.threshold.tip")}>{t("task.stt.threshold")}</span>
        <NumberField
          value={readNumber(stt, "input_threshold", 0.01)}
          step={0.01}
          onCommit={(v) => patchStt({ input_threshold: v })}
        />
      </div>
    </>
  );
}

/** One LLM_TASKS row: preset select (+ tts/stt-only extra fields), plus an
 * "unresolved" note when the task's effective preset (own assignment, or the
 * default it falls back to) doesn't resolve to anything. */
function TaskRow(props: {
  t: Translate;
  config: ConfigDocument;
  mutate: Mutate;
  taskId: LlmTaskId;
  labelKey: Parameters<Translate>[0];
  tipKey: Parameters<Translate>[0];
  presets: PresetEntry[];
}) {
  const { t, config, mutate, taskId, labelKey, tipKey, presets } = props;
  const value = taskPresetId(config, taskId);
  const resolved = resolveTaskPreset(config, taskId);

  return (
    <div class="task-model-item">
      <span data-tip={t(tipKey)}>{t(labelKey)}</span>
      <div class="task-model-fields">
        <div class="task-model-field">
          <select value={value} onChange={(e) => setTaskPreset(mutate, taskId, (e.target as HTMLSelectElement).value)}>
            <option value="">{t("task.followDefault")}</option>
            {presets.map((preset) => (
              <option key={preset.id} value={preset.id}>
                {presetLabel(preset)}
              </option>
            ))}
          </select>
        </div>
        {taskId === "tts" ? <TtsExtraFields t={t} config={config} mutate={mutate} /> : null}
        {taskId === "stt" ? <SttExtraFields t={t} config={config} mutate={mutate} /> : null}
        {!resolved ? <p class="field-hint">{t("llm.preset.unresolved")}</p> : null}
      </div>
    </div>
  );
}

export function TaskRows(props: { t: Translate; config: ConfigDocument; mutate: Mutate }) {
  const { t, config, mutate } = props;
  const presets = readPresets(config);

  return (
    <>
      <DefaultRow t={t} config={config} mutate={mutate} presets={presets} />
      {LLM_TASKS.map((task) => (
        <TaskRow
          key={task.id}
          t={t}
          config={config}
          mutate={mutate}
          taskId={task.id}
          labelKey={task.labelKey}
          tipKey={task.tipKey}
          presets={presets}
        />
      ))}
    </>
  );
}
