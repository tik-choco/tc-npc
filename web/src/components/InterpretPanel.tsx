// チャットサイドバーの「通訳」パネル: the mode switch for config.translation.mode
// and the full translation settings form (languages, context size, model,
// auto_reverse/chatbox). This used to be its own top-level tab (InterpretView)
// with the settings form living in 設定 › 通訳; that split was rejected —
// sending the user to another tab to change languages or the model
// mid-conversation defeats the point of folding 通訳 into the sidebar. The
// settings form now lives right here, in a collapsed <details> disclosure so
// it does not crowd the panel when it is not needed; the mode switch (toggled
// far more often, mid-conversation) stays permanently visible above it.
//
// This panel used to also render a live transcript of translated lines, fed
// by the WS connection. That log is gone: interpretation results now render
// inline in the チャット transcript itself (see ChatView.tsx), the same way
// the reference app agent-speech renders translations as transcript rows,
// rather than duplicating them in a second log beside it. This component is
// settings-only now.
//
// The mode switch and settings form autosave through useConfigDoc, same as
// the settings views (debounced PUT, no save button — see useConfigDoc's
// header for why). The editor itself is not created here: 音声's device
// pickers write to the same document from a sibling panel that is mounted at
// the same time, so ChatSidebar owns the sidebar's single useConfigDoc and
// passes the handle in.
import { useState } from "preact/hooks";
import { AlertTriangle, SlidersHorizontal } from "lucide-preact";

import type { ConfigDocHandle } from "../hooks/useConfigDoc";
import { useI18n } from "../hooks/useI18n";
import { SaveChip } from "./SaveChip";
import { InterpretModeField, InterpretSettings, readTranslation, translationUpdater } from "./InterpretSettings";
import "../styles/components.css";
import "../styles/interpret.css";

const SETTINGS_OPEN_STORAGE_KEY = "tc-npc:interpret-settings-open";

/** Reads whether the settings disclosure was left open from localStorage.
 *  Defensive, matching the idiom already used by ChatSidebar's
 *  loadSidebarTab() and lib/i18n.ts's detectLang(): private mode / disabled
 *  storage can throw on access, and a stale or hand-edited value might not
 *  be a recognizable boolean — either case falls back to the default.
 *
 *  That default is OPEN: the whole reason 通訳 was folded into this sidebar
 *  is that switching languages/model mid-conversation shouldn't cost a
 *  detour, and a disclosure that starts closed just reintroduces the click
 *  it was meant to remove. Anyone who would rather keep this panel compact
 *  collapses it once and the choice sticks. */
function loadSettingsOpen(): boolean {
  try {
    const raw = localStorage.getItem(SETTINGS_OPEN_STORAGE_KEY);
    if (raw === "true" || raw === "false") return raw === "true";
  } catch {
    // localStorage unavailable (private mode, etc.) — fall back to default.
  }
  return true;
}

/** Persists the settings disclosure's open/closed state. Best-effort: if the
 *  write fails the toggle still works for this session, it just won't be
 *  remembered. */
function saveSettingsOpen(open: boolean): void {
  try {
    localStorage.setItem(SETTINGS_OPEN_STORAGE_KEY, String(open));
  } catch {
    // Non-fatal — the disclosure state just won't be remembered next visit.
  }
}

export interface InterpretPanelProps {
  /** The sidebar's single config editor, shared with VoicePanel. */
  config: ConfigDocHandle;
}

export function InterpretPanel({ config: doc }: InterpretPanelProps) {
  const { t } = useI18n();
  const { config, loadError, saveState, saveError, mutate } = doc;
  const [settingsOpen, setSettingsOpen] = useState<boolean>(() => loadSettingsOpen());

  const translation = readTranslation(config);
  const update = translationUpdater(mutate);

  return (
    <div class="interpret-panel">
      {loadError && (
        <p class="interpret-panel-error">
          <AlertTriangle size={13} />
          {t("interpret.loadError")}
        </p>
      )}

      {/* The save indicator covers every field in this panel — the mode
          switch and everything inside the disclosure below — so it sits at
          the top of the panel rather than next to any one field.
          InterpretModeField brings its own `.field` wrapper and 「モード」
          label, so it must not be wrapped in a second one here. */}
      <div class="interpret-panel-head">
        <SaveChip state={saveState} error={saveError} />
      </div>

      {config ? (
        <InterpretModeField translation={translation} update={update} compact />
      ) : (
        <span class="field-hint">{t("common.loading")}</span>
      )}

      {config && (
        // Open by default: folding the settings away by default would just
        // reinstate the extra step that merging 通訳 into this sidebar was
        // meant to remove. The state persists, so collapsing it to keep the
        // panel compact is a one-time choice.
        <details
          class="interpret-panel-settings"
          open={settingsOpen}
          onToggle={(e) => {
            const open = (e.currentTarget as HTMLDetailsElement).open;
            setSettingsOpen(open);
            saveSettingsOpen(open);
          }}
        >
          <summary>
            <SlidersHorizontal size={13} />
            {t("interpret.panel.settings.summary")}
          </summary>
          <InterpretSettings translation={translation} update={update} compact />
        </details>
      )}
    </div>
  );
}
