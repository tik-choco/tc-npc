// App shell: header (brand + tab nav + theme toggle) and the single
// useNpcSocket connection, whose distributed frame state is handed down as
// props to whichever tab is active. Plain hooks only — no router/state lib
// per the app's "keep it lean" rule.
import { useState } from "preact/hooks";
import {
  Bot,
  MessageSquare,
  Users,
  UserRound,
  Mic,
  Eye,
  Gamepad2,
  CalendarClock,
  Languages,
  Brain,
  Settings as SettingsIcon,
  Moon,
  Sun,
} from "lucide-preact";

import { useTheme } from "./hooks/useTheme";
import { useI18n } from "./hooks/useI18n";
import { useNpcSocket } from "./hooks/useNpcSocket";
import type { MessageKey } from "./lib/i18n";
import { ChatView } from "./views/ChatView";
import { CharactersView } from "./views/CharactersView";
import { PeopleView } from "./views/PeopleView";
import { VoiceView } from "./views/VoiceView";
import { VisionView } from "./views/VisionView";
import { ActionView } from "./views/ActionView";
import { ScheduleView } from "./views/ScheduleView";
import { InterpretView } from "./views/InterpretView";
import { BrainView } from "./views/BrainView";
import { SettingsView } from "./views/SettingsView";

type Tab =
  | "chat"
  | "characters"
  | "people"
  | "voice"
  | "vision"
  | "action"
  | "schedule"
  | "interpret"
  | "brain"
  | "settings";

const TABS: Array<{ id: Tab; labelKey: MessageKey; icon: typeof MessageSquare }> = [
  { id: "chat", labelKey: "app.tab.chat", icon: MessageSquare },
  { id: "characters", labelKey: "app.tab.characters", icon: Users },
  { id: "people", labelKey: "app.tab.people", icon: UserRound },
  { id: "voice", labelKey: "app.tab.voice", icon: Mic },
  { id: "vision", labelKey: "app.tab.vision", icon: Eye },
  { id: "action", labelKey: "app.tab.action", icon: Gamepad2 },
  { id: "schedule", labelKey: "app.tab.schedule", icon: CalendarClock },
  { id: "interpret", labelKey: "app.tab.interpret", icon: Languages },
  { id: "brain", labelKey: "app.tab.brain", icon: Brain },
  { id: "settings", labelKey: "app.tab.settings", icon: SettingsIcon },
];

export function App() {
  const theme = useTheme();
  const { t } = useI18n();
  const [tab, setTab] = useState<Tab>("chat");
  const npc = useNpcSocket();

  return (
    <div class="app-shell">
      <header class="app-header">
        <div class="app-header-brand">
          <Bot size={20} />
          <span>TC NPC</span>
        </div>
        <nav class="app-tabs">
          {TABS.map(({ id, labelKey, icon: Icon }) => (
            <button
              key={id}
              type="button"
              class={`app-tab${tab === id ? " app-tab-active" : ""}`}
              aria-current={tab === id ? "page" : undefined}
              onClick={() => setTab(id)}
            >
              <Icon size={16} />
              <span>{t(labelKey)}</span>
            </button>
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
            onSend={(text, speaker) => npc.send({ type: "input", text, ...(speaker ? { speaker } : {}) })}
            onInterrupt={() => npc.send({ type: "interrupt" })}
          />
        )}
        {tab === "characters" && <CharactersView />}
        {tab === "people" && <PeopleView people={npc.people} peopleVersion={npc.peopleVersion} />}
        {tab === "voice" && (
          <VoiceView
            volume={npc.volume}
            ttsLines={npc.ttsLines}
            onSuspend={() => npc.send({ type: "suspend" })}
            onResume={() => npc.send({ type: "resume" })}
          />
        )}
        {tab === "vision" && <VisionView visionLog={npc.visionLog} />}
        {tab === "action" && (
          <ActionView
            position={npc.position}
            actionLogEntries={npc.actionLogEntries}
            onCommand={(text) => npc.send({ type: "command", text })}
          />
        )}
        {tab === "schedule" && <ScheduleView />}
        {tab === "interpret" && <InterpretView translations={npc.translations} />}
        {tab === "brain" && (
          <BrainView affect={npc.affect} affectHistory={npc.affectHistory} memoryVersion={npc.memoryVersion} />
        )}
        {tab === "settings" && <SettingsView />}
      </main>
    </div>
  );
}
