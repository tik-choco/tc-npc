// PresetCards — 「モデル」カードグリッド（設定 › AI接続タブの後半）。
// tc-docs/drafts/llm-settings-common-v1.md §3.1 の
// 「モデル（preset）: フラットなカードリスト + 追加タイル。label / 接続先
// select / モデル select（fetch 失敗時は手入力にフォールバック）/ temperature
// [このアプリでは reasoning_effort]。カードにバッジ表示: 既定 / 各タスク
// 割当 / …」を実装したもの。
//
// 移植元は tc-translate の src/components/SettingsModal.tsx の
// renderModelRow / renderAddModelRow / renderAddModelTile。CSS クラス名
// （model-row 系、../styles/settings-llm.css に移植済み）はそのまま踏襲。
// モデル一覧の fetch とその手入力フォールバックは SettingsFields.tsx の
// ModelPicker（POST /api/llm/models、fetch 自体が接続テストを兼ねる規約）
// に委譲している。
//
// ProviderCards との違い: この設定共通仕様には mist-network:// 疑似
// プロバイダ（§2.2）や AI Network 由来カードの強調表示（.model-row-network）
// があるが、tc-npc はまだ AI Network タブ自体を持たないため、それらは
// 実装していない（他 worker が AI Network タブを追加する際に再検討）。
import { useEffect, useRef, useState } from "preact/hooks";
import { Plus } from "lucide-preact";
import "../styles/settings-llm.css";
import type { Translate } from "../lib/i18n";
import type { ConfigDocument } from "../lib/types";
import type { PresetEntry } from "../lib/config-types";
import type { Mutate } from "../lib/llm-config";
import {
  addPreset,
  deletePreset,
  newId,
  presetBadgeKeys,
  presetLabel,
  providerLabel,
  providerOf,
  readDefaultPresetId,
  readPresets,
  readProviders,
  setDefaultPreset,
  updatePreset,
} from "../lib/llm-config";
import { PRESET_EFFORT_OPTIONS, ModelPicker, SelectField, TextField, ToggleField } from "./SettingsFields";

export interface PresetCardsProps {
  t: Translate;
  config: ConfigDocument;
  mutate: Mutate;
}

export function PresetCards({ t, config, mutate }: PresetCardsProps) {
  const providers = readProviders(config);
  const presets = readPresets(config);
  const defaultPresetId = readDefaultPresetId(config);

  // Only one inline row (an existing preset's edit form, or the add tile's
  // form) is ever open at a time — same rule as ProviderCards, kept
  // independent per component (see that file's header for why cross
  // -component exclusivity isn't required).
  const [editingId, setEditingId] = useState("");
  const [adding, setAdding] = useState(false);
  // Draft for the add form only — a new preset has nowhere to live in
  // config.presets until "追加" is pressed, so its fields need local state
  // (mirrors ProviderCards' addLabel/addBaseUrl/addApiKey).
  const [addLabel, setAddLabel] = useState("");
  const [addProviderId, setAddProviderId] = useState("");
  const [addModel, setAddModel] = useState("");
  const [addEffort, setAddEffort] = useState("");

  function closeAll(): void {
    setEditingId("");
    setAdding(false);
  }

  // If the preset currently being edited disappears (deleted from this same
  // grid, or from another tab/app sharing the same config), close its row
  // instead of leaving it editing a value that no longer exists.
  useEffect(() => {
    if (editingId && !presets.some((p) => p.id === editingId)) setEditingId("");
  }, [presets, editingId]);

  // Same guard for the add form's 接続先 pick. ProviderCards is mounted
  // alongside this grid on the AI接続 tab, so a provider can be deleted while
  // this form sits open — and its × handler calls stopPropagation, so the
  // outside-click listener above never fires for it. Without this the form
  // would happily save a preset pointing at a provider that no longer
  // exists, and nothing prunes that afterwards (pruning only runs on
  // provider/preset deletion).
  useEffect(() => {
    if (!adding || !addProviderId) return;
    if (!providers.some((p) => p.id === addProviderId)) setAddProviderId(providers[0]?.id ?? "");
  }, [providers, adding, addProviderId]);

  // Outside-click / Escape closes whichever row is open. Identical pattern
  // to ProviderCards — see its file header for the mousedown-before-click
  // rationale.
  const activeRowRef = useRef<HTMLDivElement | null>(null);
  const mouseDownInsideRef = useRef(false);
  useEffect(() => {
    if (!editingId && !adding) return undefined;

    function handleDocumentMouseDown(event: MouseEvent): void {
      mouseDownInsideRef.current = Boolean(
        activeRowRef.current && activeRowRef.current.contains(event.target as Node),
      );
    }
    function handleDocumentClick(event: MouseEvent): void {
      if (activeRowRef.current && activeRowRef.current.contains(event.target as Node)) return;
      if (mouseDownInsideRef.current) return;
      closeAll();
    }
    function handleKeyDown(event: KeyboardEvent): void {
      if (event.key === "Escape") closeAll();
    }

    document.addEventListener("mousedown", handleDocumentMouseDown);
    document.addEventListener("click", handleDocumentClick);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handleDocumentMouseDown);
      document.removeEventListener("click", handleDocumentClick);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [editingId, adding]);

  function openEdit(id: string): void {
    closeAll();
    setEditingId(id);
  }

  function openAdd(): void {
    closeAll();
    setAdding(true);
    setAddLabel("");
    // Default to the first provider rather than leaving the picker
    // unselected: there is no "follow default"-style placeholder option for
    // 接続先 the way task assignments have one, and the reference
    // implementation's unselected placeholder text has no equivalent key in
    // this app's i18n. Since providers.length === 0 already disables the
    // add tile entirely (see the bottom of this file), there is always at
    // least one provider to default to here.
    setAddProviderId(providers[0]?.id ?? "");
    setAddModel("");
    setAddEffort("");
  }

  function saveAdd(): void {
    const model = addModel.trim();
    // The provider must still exist — the last provider can be deleted while
    // this form is open (see the effect above).
    if (!model || !providers.some((p) => p.id === addProviderId)) return;
    addPreset(mutate, {
      id: newId("m", presets.map((p) => p.id)),
      label: addLabel.trim(),
      provider_id: addProviderId,
      model,
      reasoning_effort: addEffort,
    });
    setAdding(false);
  }

  function removePreset(preset: PresetEntry): void {
    const ok = window.confirm(t("llm.preset.deleteConfirm", { label: presetLabel(preset) }));
    if (!ok) return;
    deletePreset(mutate, preset.id);
    if (editingId === preset.id) setEditingId("");
  }

  function renderCard(preset: PresetEntry) {
    if (editingId === preset.id) {
      const provider = providerOf(config, preset);
      const badges = presetBadgeKeys(config, preset.id);
      const isDefault = preset.id === defaultPresetId;
      return (
        <div class="model-row model-row-editing" key={preset.id} ref={activeRowRef}>
          <div class="model-row-edit-fields">
            <TextField
              label={t("llm.preset.label")}
              value={preset.label ?? ""}
              placeholder={t("llm.preset.label.placeholder")}
              onCommit={(v) => updatePreset(mutate, preset.id, { label: v })}
            />
            <SelectField
              label={t("llm.preset.provider")}
              value={preset.provider_id ?? ""}
              options={providers.map((p) => ({ value: p.id, label: providerLabel(p) }))}
              // Switching providers commits immediately and clears the
              // stored model in the same patch — a model name from the old
              // provider's list is meaningless against the new provider, so
              // the field is left for the user to pick again (matches the
              // shared spec's "接続先を変えたらモデル欄はリセットして選び直
              // させる").
              onChange={(v) => updatePreset(mutate, preset.id, { provider_id: v, model: "" })}
              hint={!provider ? t("llm.preset.unresolved") : undefined}
            />
            <label class="field">
              <span>{t("settings.field.model")}</span>
              <ModelPicker
                t={t}
                value={preset.model ?? ""}
                placeholder="gpt-4o-mini"
                baseUrl={provider?.base_url ?? ""}
                apiKey={provider?.api_key ?? ""}
                section="api"
                kind="models"
                itemLabel={t("picker.item.model")}
                providerId={provider?.id}
                onChange={(v) => updatePreset(mutate, preset.id, { model: v })}
              />
            </label>
            <label class="field" title={t("settings.effort.tooltip")}>
              <span>{t("settings.effort.label")}</span>
              <select
                value={preset.reasoning_effort ?? ""}
                onChange={(e) =>
                  updatePreset(mutate, preset.id, { reasoning_effort: (e.target as HTMLSelectElement).value })
                }
              >
                {PRESET_EFFORT_OPTIONS.map((opt) => (
                  <option key={opt.value} value={opt.value}>
                    {t(opt.labelKey)}
                  </option>
                ))}
              </select>
            </label>
            <ToggleField
              label={t("llm.badge.default")}
              checked={isDefault}
              onChange={(checked) => setDefaultPreset(mutate, checked ? preset.id : "")}
            />
            {badges.length > 0 ? (
              <span class="model-row-badges">
                {badges.map((key) => (
                  <span key={key} class="task-badge">
                    {t(key)}
                  </span>
                ))}
              </span>
            ) : null}
          </div>
        </div>
      );
    }

    const provider = providerOf(config, preset);
    const badges = presetBadgeKeys(config, preset.id);
    return (
      <div class="model-row" key={preset.id}>
        <button type="button" class="model-row-main" onClick={() => openEdit(preset.id)}>
          <span class="model-row-label">{presetLabel(preset)}</span>
          <span class="model-row-model">{preset.model || "—"}</span>
          <span class="model-row-provider">{provider ? providerLabel(provider) : t("llm.preset.unresolved")}</span>
        </button>
        {badges.length > 0 ? (
          <span class="model-row-badges">
            {badges.map((key) => (
              <span key={key} class="task-badge">
                {t(key)}
              </span>
            ))}
          </span>
        ) : null}
        <span
          class="preset-chip-remove model-row-remove"
          role="button"
          tabIndex={0}
          title={t("llm.preset.delete")}
          onClick={(event) => {
            event.stopPropagation();
            removePreset(preset);
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              event.stopPropagation();
              removePreset(preset);
            }
          }}
        >
          ×
        </span>
      </div>
    );
  }

  function renderAddForm() {
    const provider = providers.find((p) => p.id === addProviderId);
    return (
      <div class="model-row model-row-editing model-row-add" ref={activeRowRef}>
        <div class="model-row-edit-fields">
          <TextField
            label={t("llm.preset.label")}
            value={addLabel}
            placeholder={t("llm.preset.label.placeholder")}
            onCommit={setAddLabel}
          />
          <SelectField
            label={t("llm.preset.provider")}
            value={addProviderId}
            options={providers.map((p) => ({ value: p.id, label: providerLabel(p) }))}
            onChange={(v) => {
              setAddProviderId(v);
              setAddModel("");
            }}
          />
          <label class="field">
            <span>{t("settings.field.model")}</span>
            <ModelPicker
              t={t}
              value={addModel}
              placeholder="gpt-4o-mini"
              baseUrl={provider?.base_url ?? ""}
              apiKey={provider?.api_key ?? ""}
              section="api"
              kind="models"
              itemLabel={t("picker.item.model")}
              providerId={provider?.id}
              onChange={setAddModel}
            />
          </label>
          <label class="field" title={t("settings.effort.tooltip")}>
            <span>{t("settings.effort.label")}</span>
            <select value={addEffort} onChange={(e) => setAddEffort((e.target as HTMLSelectElement).value)}>
              {PRESET_EFFORT_OPTIONS.map((opt) => (
                <option key={opt.value} value={opt.value}>
                  {t(opt.labelKey)}
                </option>
              ))}
            </select>
          </label>
        </div>
        <div class="model-row-add-actions">
          <button
            type="button"
            class="connection-form-btn connection-form-btn-primary"
            onClick={saveAdd}
            disabled={!addProviderId || !addModel.trim()}
          >
            <Plus size={13} />
            {t("llm.add")}
          </button>
          <button type="button" class="connection-form-btn" onClick={() => setAdding(false)}>
            {t("llm.cancel")}
          </button>
        </div>
      </div>
    );
  }

  return (
    <>
      <div class="server-list-header">
        {/* See ProviderCards for why `title` rides along with `data-tip`
            here: settings-llm.css only wires the hover-reveal ::after rule
            up for .task-model-item's labels today, not this header. */}
        <label data-tip={t("llm.presets.tip")} title={t("llm.presets.tip")}>
          {t("llm.presets.title")}
        </label>
      </div>
      <div class="settings-flat-section settings-flat-section-models">
        {presets.length === 0 && !adding ? <p class="field-hint">{t("llm.presets.empty")}</p> : null}
        <div class="model-row-list">
          {presets.map((preset) => renderCard(preset))}
          {adding ? (
            renderAddForm()
          ) : providers.length === 0 ? (
            <button type="button" class="grid-add-tile" disabled title={t("llm.presets.needProvider")}>
              <Plus size={14} />
              <span>{t("llm.presets.add")}</span>
            </button>
          ) : (
            <button type="button" class="grid-add-tile" onClick={openAdd}>
              <Plus size={14} />
              <span>{t("llm.presets.add")}</span>
            </button>
          )}
        </div>
      </div>
    </>
  );
}
