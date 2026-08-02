// App shell: header (brand + tab nav + theme toggle) and the single
// useNpcSocket connection, whose distributed frame state is handed down as
// props to whichever tab is active. Plain hooks only — no router/state lib
// per the app's "keep it lean" rule. The top-level nav is チャット(+音声/
// 状態/通訳 サイドパネル) / キャラ / 人物 / 視覚 / 行動 / 予定 / 感情 /
// 設定 — 音声 and 通訳 no longer have their own top-level tabs; they live
// inside the チャット tab's sidebar (see ChatSidebar / ChatView) alongside
// the 状態 panel.
//
// Which tab is active lives in the URL fragment (`#/brain`, `#/chat/status`)
// rather than in component state — see lib/router.ts for why the fragment and
// not a path. That makes each tab separately addressable, so the operator can
// keep several windows open on different tabs at once; the header nav is
// therefore rendered as real links, which ctrl/middle-click opens in a new
// window for free.
import { useEffect } from "preact/hooks";
import {
  Bot,
  MessageSquare,
  Users,
  UserRound,
  Eye,
  Gamepad2,
  CalendarClock,
  Brain,
  Settings as SettingsIcon,
  Moon,
  Sun,
} from "lucide-preact";

import { useTheme } from "./hooks/useTheme";
import { useI18n } from "./hooks/useI18n";
import { useNpcSocket } from "./hooks/useNpcSocket";
import type { MessageKey } from "./lib/i18n";
import { formatHash, useRoute, type Tab } from "./lib/router";
import { ChatView } from "./views/ChatView";
import { AvatarView } from "./views/AvatarView";
import { CharactersView } from "./views/CharactersView";
import { PeopleView } from "./views/PeopleView";
import { VisionView } from "./views/VisionView";
import { ActionView } from "./views/ActionView";
import { ScheduleView } from "./views/ScheduleView";
import { BrainView } from "./views/BrainView";
import { SettingsView } from "./views/SettingsView";

const TABS: Array<{ id: Tab; labelKey: MessageKey; icon: typeof MessageSquare }> = [
  { id: "chat", labelKey: "app.tab.chat", icon: MessageSquare },
  { id: "characters", labelKey: "app.tab.characters", icon: Users },
  { id: "people", labelKey: "app.tab.people", icon: UserRound },
  { id: "vision", labelKey: "app.tab.vision", icon: Eye },
  { id: "action", labelKey: "app.tab.action", icon: Gamepad2 },
  { id: "schedule", labelKey: "app.tab.schedule", icon: CalendarClock },
  { id: "brain", labelKey: "app.tab.brain", icon: Brain },
  { id: "settings", labelKey: "app.tab.settings", icon: SettingsIcon },
];

export function App() {
  const theme = useTheme();
  const { t } = useI18n();
  const { route, setChatPanel, setChatLayout } = useRoute();
  const tab = route.tab;
  const npc = useNpcSocket();

  // Name the window after its tab. With several windows open on different
  // tabs — the whole point of routing them — the title bar and taskbar entry
  // are the only thing that tells them apart.
  useEffect(() => {
    const label = TABS.find((entry) => entry.id === tab)?.labelKey;
    document.title = tab === "avatar" ? `TC NPC — ${t("avatar.window.title")}` : label ? `TC NPC — ${t(label)}` : "TC NPC";
  }, [tab, t]);

  // The bare avatar window is deliberately chrome-free: it exists to be
  // dragged onto a second monitor or pointed at by capture software, and a
  // header with a tab bar in the shot would defeat that. It is therefore
  // returned before the shell rather than inside it.
  if (tab === "avatar") {
    return (
      <AvatarView
        character={npc.character}
        avatar={npc.avatar}
        speaking={npc.speaking}
        speakingLevelRef={npc.speakingLevelRef}
        // The bare window has no toolbar of its own, so its status pill is
        // the only thing there telling you whether the NPC is listening,
        // thinking or talking — it needs the same three signals the チャット
        // toolbar derives that from.
        voiceActive={npc.voiceActive}
        volume={npc.volume}
        pending={npc.pending}
        affect={npc.affect}
        ttsLines={npc.ttsLines}
      />
    );
  }

  return (
    <div class="app-shell">
      <header class="app-header">
        <div class="app-header-brand">
          <Bot size={20} />
          <span>TC NPC</span>
        </div>
        <nav class="app-tabs">
          {/* Real links, not buttons: activating one only has to change the
              fragment, which useRoute() picks up — and ctrl/middle-click then
              opens that tab in its own window without any extra handling. The
              チャット link keeps the current sidebar panel so switching away
              and back doesn't reset it. */}
          {TABS.map(({ id, labelKey, icon: Icon }) => (
            <a
              key={id}
              href={formatHash({ ...route, tab: id })}
              class={`app-tab${tab === id ? " app-tab-active" : ""}`}
              aria-current={tab === id ? "page" : undefined}
            >
              <Icon size={16} />
              <span>{t(labelKey)}</span>
            </a>
          ))}
        </nav>
        <div class="app-header-links">
          <button
            type="button"
            class="theme-toggle"
            onClick={theme.toggleTheme}
            aria-label={theme.theme === "light" ? t("app.theme.toDark") : t("app.theme.toLight")}
          >
            {theme.theme === "light" ? <Moon size={18} /> : <Sun size={18} />}
          </button>
        </div>
      </header>

      <main class="app-main">
        {tab === "chat" && (
          <ChatView
            entries={npc.timeline}
            errors={npc.errors}
            pending={npc.pending}
            connectionState={npc.connectionState}
            version={npc.version}
            character={npc.character}
            avatar={npc.avatar}
            modules={npc.modules}
            affect={npc.affect}
            volume={npc.volume}
            ttsLines={npc.ttsLines}
            translations={npc.translations}
            voiceActive={npc.voiceActive}
            ttsSuspended={npc.ttsSuspended}
            speaking={npc.speaking}
            speakingLevelRef={npc.speakingLevelRef}
            sidebarPanel={route.chatPanel}
            onSidebarPanelChange={setChatPanel}
            layout={route.chatLayout}
            onLayoutChange={setChatLayout}
            onSend={(text, speaker) => npc.send({ type: "input", text, speaker })}
            onInterrupt={() => npc.send({ type: "interrupt" })}
            onSuspend={() => npc.send({ type: "suspend" })}
            onResume={() => npc.send({ type: "resume" })}
            onVoiceStart={() => npc.send({ type: "voiceStart" })}
            onVoiceStop={() => npc.send({ type: "voiceStop" })}
          />
        )}
        {tab === "characters" && <CharactersView />}
        {tab === "people" && <PeopleView people={npc.people} peopleVersion={npc.peopleVersion} />}
        {tab === "vision" && <VisionView visionLog={npc.visionLog} />}
        {tab === "action" && (
          <ActionView
            position={npc.position}
            actionLogEntries={npc.actionLogEntries}
            onCommand={(text) => npc.send({ type: "command", text })}
          />
        )}
        {tab === "schedule" && (
          <ScheduleView speaking={npc.speaking} onInterrupt={() => npc.send({ type: "interrupt" })} />
        )}
        {tab === "brain" && (
          <BrainView affect={npc.affect} affectHistory={npc.affectHistory} memoryVersion={npc.memoryVersion} />
        )}
        {tab === "settings" && <SettingsView />}
      </main>
    </div>
  );
}
