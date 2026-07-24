// App shell: header (brand + tab nav + theme toggle) and the single
// useNpcSocket connection, whose distributed frame state is handed down as
// props to whichever tab is active. Plain hooks only — no router/state lib
// per the app's "keep it lean" rule.
import { useState } from "preact/hooks";
import { Bot, MessageSquare, Users, Mic, Eye, Gamepad2, Settings as SettingsIcon, Moon, Sun } from "lucide-preact";

import { useTheme } from "./hooks/useTheme";
import { useNpcSocket } from "./hooks/useNpcSocket";
import { ChatView } from "./views/ChatView";
import { CharactersView } from "./views/CharactersView";
import { VoiceView } from "./views/VoiceView";
import { VisionView } from "./views/VisionView";
import { ActionView } from "./views/ActionView";
import { SettingsView } from "./views/SettingsView";

type Tab = "chat" | "characters" | "voice" | "vision" | "action" | "settings";

const TABS: Array<{ id: Tab; label: string; icon: typeof MessageSquare }> = [
  { id: "chat", label: "チャット", icon: MessageSquare },
  { id: "characters", label: "キャラ", icon: Users },
  { id: "voice", label: "音声", icon: Mic },
  { id: "vision", label: "視覚", icon: Eye },
  { id: "action", label: "行動", icon: Gamepad2 },
  { id: "settings", label: "設定", icon: SettingsIcon },
];

export function App() {
  const theme = useTheme();
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
          {TABS.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              type="button"
              class={`app-tab${tab === id ? " app-tab-active" : ""}`}
              aria-current={tab === id ? "page" : undefined}
              onClick={() => setTab(id)}
            >
              <Icon size={16} />
              <span>{label}</span>
            </button>
          ))}
        </nav>
        <div class="app-header-links">
          <button
            type="button"
            class="theme-toggle"
            onClick={theme.toggleTheme}
            aria-label={theme.theme === "light" ? "ダークモードに切り替え" : "ライトモードに切り替え"}
          >
            {theme.theme === "light" ? <Moon size={18} /> : <Sun size={18} />}
          </button>
        </div>
      </header>

      <main class="app-main">
        {tab === "chat" && (
          <ChatView
            entries={npc.timeline}
            connectionState={npc.connectionState}
            onSend={(text) => npc.send({ type: "input", text })}
            onInterrupt={() => npc.send({ type: "interrupt" })}
          />
        )}
        {tab === "characters" && <CharactersView />}
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
        {tab === "settings" && <SettingsView />}
      </main>
    </div>
  );
}
