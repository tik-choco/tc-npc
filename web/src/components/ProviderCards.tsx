// ProviderCards — 「接続先」カードグリッド（設定 › AI接続タブの前半）。
// tc-docs/drafts/llm-settings-common-v1.md §3.1 の
// 「接続先（provider）: フラットなカードリスト + 追加タイル。インライン編集
// （label / baseUrl / apiKey、blur でコミット、外側クリック/Escape で閉じる）」
// を実装したもの。
//
// 移植元は tc-translate の src/components/SettingsModal.tsx の
// renderProviderRow / renderAddProviderRow / renderAddProviderTile。CSS
// クラス名（model-row / model-row-editing / model-row-add / grid-add-tile
// など、../styles/settings-llm.css に移植済み）はそのまま踏襲している。
// このリポジトリでは「接続先」と「モデル」が2つの独立コンポーネント
// （このファイルと PresetCards.tsx）に分かれている点だけが構成上の違いで、
// インライン編集の開閉ロジック（1行だけ開く・外側クリック/Escapeで閉じる・
// 対象が消えたら自動で閉じる）は各コンポーネント内で独立して持てば足りる
// （コンポーネント間の排他までは要求されていない）。
//
// 接続テスト（モデル一覧 fetch を接続確認として兼ねる規約、§3.1）は
// PresetCards 側の ModelPicker が担う。ここでは base_url / api_key を保存
// するだけで、実際に接続できるかどうかはモデルを選ぶときに確認される。
import { useEffect, useRef, useState } from "preact/hooks";
import { Plus } from "lucide-preact";
import "../styles/settings-llm.css";
import type { Translate } from "../lib/i18n";
import type { ConfigDocument } from "../lib/types";
import type { ProviderEntry } from "../lib/config-types";
import type { Mutate } from "../lib/llm-config";
import {
  addProvider,
  deleteProvider,
  newId,
  presetCountForProvider,
  providerLabel,
  readProviders,
  updateProvider,
} from "../lib/llm-config";
import { TextField } from "./SettingsFields";

export interface ProviderCardsProps {
  t: Translate;
  config: ConfigDocument;
  mutate: Mutate;
}

export function ProviderCards({ t, config, mutate }: ProviderCardsProps) {
  const providers = readProviders(config);

  // Only one inline row (an existing provider's edit form, or the add
  // tile's form) is ever open at a time; opening one closes whichever was
  // open before.
  const [editingId, setEditingId] = useState("");
  const [adding, setAdding] = useState(false);
  // Draft for the add form only. Editing an *existing* provider writes
  // straight through TextField's onCommit into config (see renderCard
  // below), but a new provider has nowhere to live in config.providers
  // until "追加" is actually pressed, so its fields need local state.
  const [addLabel, setAddLabel] = useState("");
  const [addBaseUrl, setAddBaseUrl] = useState("");
  const [addApiKey, setAddApiKey] = useState("");

  function closeAll(): void {
    setEditingId("");
    setAdding(false);
  }

  // If the provider currently being edited disappears (deleted from this
  // same card grid via cascading delete of a different row's confirm, or
  // from another tab/app sharing the same config), close its row instead of
  // leaving it editing a value that no longer exists.
  useEffect(() => {
    if (editingId && !providers.some((p) => p.id === editingId)) setEditingId("");
  }, [providers, editingId]);

  // Outside-click / Escape closes whichever row is open. Mirrors
  // PresetCards' identical handler — see its file header for the
  // mousedown-before-click rationale (guards against a text-selection drag
  // that starts inside the row and ends outside it being misread as an
  // "outside click").
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
    setAddBaseUrl("");
    setAddApiKey("");
  }

  function saveAdd(): void {
    const baseUrl = addBaseUrl.trim();
    if (!baseUrl) return;
    addProvider(mutate, {
      id: newId("p", providers.map((p) => p.id)),
      label: addLabel.trim(),
      base_url: baseUrl,
      api_key: addApiKey,
    });
    setAdding(false);
  }

  function removeProvider(provider: ProviderEntry): void {
    // presetCountForProvider is only surfaced as an informational badge on
    // the card (below) — deleteProvider itself already cascades (removes
    // any preset that referenced this provider and clears dangling task
    // assignments), so a single confirm covers the whole operation
    // regardless of how many models are attached.
    const ok = window.confirm(t("llm.provider.deleteConfirm", { label: providerLabel(provider) }));
    if (!ok) return;
    deleteProvider(mutate, provider.id);
    if (editingId === provider.id) setEditingId("");
  }

  function renderCard(provider: ProviderEntry) {
    if (editingId === provider.id) {
      return (
        <div class="model-row model-row-editing" key={provider.id} ref={activeRowRef}>
          <div class="model-row-edit-fields">
            <TextField
              label={t("llm.provider.label")}
              value={provider.label ?? ""}
              placeholder={t("llm.provider.label.placeholder")}
              onCommit={(v) => updateProvider(mutate, provider.id, { label: v })}
            />
            <TextField
              label={t("settings.field.baseUrl")}
              tooltip={t("settings.field.baseUrl.tooltip")}
              value={provider.base_url ?? ""}
              placeholder="http://localhost:11434/v1"
              onCommit={(v) => updateProvider(mutate, provider.id, { base_url: v })}
            />
            <TextField
              label={t("settings.field.apiKey")}
              tooltip={t("settings.field.apiKey.tooltip")}
              type="password"
              value={provider.api_key ?? ""}
              placeholder="sk-..."
              onCommit={(v) => updateProvider(mutate, provider.id, { api_key: v })}
            />
          </div>
        </div>
      );
    }

    const count = presetCountForProvider(config, provider.id);
    return (
      <div class="model-row" key={provider.id}>
        <button type="button" class="model-row-main" onClick={() => openEdit(provider.id)}>
          <span class="model-row-label">{providerLabel(provider)}</span>
          <span class="model-row-model">{provider.base_url || "—"}</span>
        </button>
        <span class="model-row-badges">
          {count > 0 ? <span class="task-badge">{t("llm.provider.presetCount", { count })}</span> : null}
        </span>
        <span
          class="preset-chip-remove model-row-remove"
          role="button"
          tabIndex={0}
          title={t("llm.provider.delete")}
          onClick={(event) => {
            event.stopPropagation();
            removeProvider(provider);
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" || event.key === " ") {
              event.preventDefault();
              event.stopPropagation();
              removeProvider(provider);
            }
          }}
        >
          ×
        </span>
      </div>
    );
  }

  function renderAddForm() {
    return (
      <div class="model-row model-row-editing model-row-add" ref={activeRowRef}>
        <div class="model-row-edit-fields">
          <TextField
            label={t("llm.provider.label")}
            value={addLabel}
            placeholder={t("llm.provider.label.placeholder")}
            onCommit={setAddLabel}
          />
          <TextField
            label={t("settings.field.baseUrl")}
            tooltip={t("settings.field.baseUrl.tooltip")}
            value={addBaseUrl}
            placeholder="http://localhost:11434/v1"
            onCommit={setAddBaseUrl}
          />
          <TextField
            label={t("settings.field.apiKey")}
            tooltip={t("settings.field.apiKey.tooltip")}
            type="password"
            value={addApiKey}
            placeholder="sk-..."
            onCommit={setAddApiKey}
          />
        </div>
        <div class="model-row-add-actions">
          <button
            type="button"
            class="connection-form-btn connection-form-btn-primary"
            onClick={saveAdd}
            disabled={!addBaseUrl.trim()}
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
        {/* data-tip per the shared spec's tooltip convention; settings-llm.css
            currently only wires up the hover-reveal ::after rule for
            .task-model-item's labels (the タスク tab rows), not this header,
            so `title` rides along as a working fallback until/unless that
            CSS is generalized. */}
        <label data-tip={t("llm.providers.tip")} title={t("llm.providers.tip")}>
          {t("llm.providers.title")}
        </label>
      </div>
      <div class="settings-flat-section settings-flat-section-connection">
        {providers.length === 0 && !adding ? <p class="field-hint">{t("llm.providers.empty")}</p> : null}
        <div class="model-row-list">
          {providers.map((provider) => renderCard(provider))}
          {adding ? (
            renderAddForm()
          ) : (
            <button type="button" class="grid-add-tile" onClick={openAdd}>
              <Plus size={14} />
              <span>{t("llm.providers.add")}</span>
            </button>
          )}
        </div>
      </div>
    </>
  );
}
