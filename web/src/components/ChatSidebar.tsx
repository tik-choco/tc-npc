// チャット tab's right-hand sidebar: a 320px pill-tabbed column that used to
// be the standalone 音声 tab and is now merged alongside a 状態 (内心 affect
// summary + connection/version/character/modules) tab and a 通訳
// (interpretation settings) tab.
// Ported from agent-speech's SidebarTabs pattern (see agent-speech/web/src/
// components/SidebarTabs.tsx) — same pill segmented control, same "keep every
// panel mounted, toggle `hidden`" approach.
//
// The active pill is controlled by the router rather than owned here: it
// rides the URL as `#/chat/<panel>` (see lib/router.ts) so two windows can
// show different panels side by side. lib/router.ts also keeps the
// localStorage fallback this component used to own.
//
// The config editor is owned here rather than inside a panel: 音声 (device
// pickers) and 通訳 (mode + translation settings) both write to the config
// document and both are permanently mounted, and a mounted tree may hold at
// most one useConfigDoc (see its header) — two would each PUT their own
// snapshot and silently roll back the other's field. So ChatSidebar holds
// the single editor and hands the handle to both panels.
import { Mic, Activity, Languages } from "lucide-preact";
import type { AffectSnapshot, TtsLineEntry } from "../hooks/useNpcSocket";
import type { ConnectionState } from "../lib/ws";
import type { ChatPanel } from "../lib/router";
import { useConfigDoc } from "../hooks/useConfigDoc";
import { useI18n } from "../hooks/useI18n";
import type { Translate } from "../lib/i18n";
import { VoicePanel } from "./VoicePanel";
import { StatusPanel } from "./StatusPanel";
import { InterpretPanel } from "./InterpretPanel";
import "../styles/sidebar.css";

export interface ChatSidebarProps {
  connectionState: ConnectionState;
  version: string | null;
  character: { id: string; name: string } | null;
  modules: Record<string, boolean>;
  /** Forwarded to StatusPanel's 内心 card — the condensed affect readout that
   *  saves a trip to the 感情 tab mid-conversation. */
  affect: AffectSnapshot | null;
  volume: number;
  ttsLines: TtsLineEntry[];
  /** Forwarded to VoicePanel so the mic meter can explain a zero reading
   *  while the チャット header's 音声 master switch is off. */
  voiceActive: boolean;
  /** Whether the server acknowledged the last 一時停止/再開 this UI sent, so
   *  those buttons can stop being fire-and-forget. Null until one is sent. */
  ttsSuspended: boolean | null;
  /** Active pill, owned by the router (`#/chat/<panel>`). */
  activePanel: ChatPanel;
  onPanelChange: (panel: ChatPanel) => void;
  onSuspend: () => void;
  onResume: () => void;
}

function sidebarTabs(t: Translate): Array<{ id: ChatPanel; label: string; icon: typeof Mic }> {
  return [
    { id: "voice", label: t("chat.sidebar.tab.voice"), icon: Mic },
    { id: "status", label: t("chat.sidebar.tab.status"), icon: Activity },
    { id: "interpret", label: t("chat.sidebar.tab.interpret"), icon: Languages },
  ];
}

export function ChatSidebar({
  connectionState,
  version,
  character,
  modules,
  affect,
  volume,
  ttsLines,
  voiceActive,
  ttsSuspended,
  activePanel,
  onPanelChange,
  onSuspend,
  onResume,
}: ChatSidebarProps) {
  const { t } = useI18n();
  const config = useConfigDoc();

  return (
    <>
      <div class="chat-sidebar-tabs" role="tablist" aria-label={t("chat.sidebar.label")}>
        {sidebarTabs(t).map((tab) => {
          const Icon = tab.icon;
          const selected = activePanel === tab.id;
          return (
            <button
              key={tab.id}
              type="button"
              role="tab"
              id={`chat-sidebar-tab-${tab.id}`}
              aria-selected={selected}
              aria-controls={`chat-sidebar-tabpanel-${tab.id}`}
              class={`chat-sidebar-tab${selected ? " chat-sidebar-tab--active" : ""}`}
              onClick={() => onPanelChange(tab.id)}
            >
              <Icon size={15} />
              {tab.label}
            </button>
          );
        })}
      </div>

      {/* All three panels stay mounted regardless of which tab is active:
          the volume meter and TTS log inside VoicePanel need to keep
          receiving frames (volume updates, new ttsLines) even while another
          tab is showing, so a viewer who switches back sees a live meter and
          a complete log rather than one that resumed from empty. We only
          toggle `hidden` on the wrapping panel instead of conditionally
          rendering. */}
      <div
        class="chat-sidebar-panel"
        role="tabpanel"
        id="chat-sidebar-tabpanel-voice"
        aria-labelledby="chat-sidebar-tab-voice"
        hidden={activePanel !== "voice"}
      >
        <VoicePanel
          volume={volume}
          ttsLines={ttsLines}
          voiceActive={voiceActive}
          ttsSuspended={ttsSuspended}
          config={config}
          onSuspend={onSuspend}
          onResume={onResume}
        />
      </div>

      <div
        class="chat-sidebar-panel"
        role="tabpanel"
        id="chat-sidebar-tabpanel-status"
        aria-labelledby="chat-sidebar-tab-status"
        hidden={activePanel !== "status"}
      >
        <StatusPanel
          connectionState={connectionState}
          version={version}
          character={character}
          modules={modules}
          affect={affect}
        />
      </div>

      <div
        class="chat-sidebar-panel"
        role="tabpanel"
        id="chat-sidebar-tabpanel-interpret"
        aria-labelledby="chat-sidebar-tab-interpret"
        hidden={activePanel !== "interpret"}
      >
        <InterpretPanel config={config} />
      </div>
    </>
  );
}
