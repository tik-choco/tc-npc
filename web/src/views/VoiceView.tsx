// 音声 tab: live volume meter, suspend/resume controls, and a log of
// ttsLine frames (spoken lines + any translations).
import { Pause, Play, Volume2, Languages } from "lucide-preact";
import type { TtsLineEntry } from "../hooks/useNpcSocket";
import { useI18n } from "../hooks/useI18n";
import "../styles/components.css";
import "../styles/voice.css";

function formatTime(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(
    d.getSeconds(),
  ).padStart(2, "0")}`;
}

export interface VoiceViewProps {
  volume: number;
  ttsLines: TtsLineEntry[];
  onSuspend: () => void;
  onResume: () => void;
}

export function VoiceView({ volume, ttsLines, onSuspend, onResume }: VoiceViewProps) {
  const { t } = useI18n();
  const pct = Math.max(0, Math.min(1, volume)) * 100;

  return (
    <div class="voice-view">
      <section class="voice-meter-card">
        <div class="voice-meter-header">
          <Volume2 size={16} />
          <span>{t("voice.micVolume")}</span>
          <span class="voice-meter-value">{pct.toFixed(0)}%</span>
        </div>
        <div class="voice-meter-bar">
          <div class="voice-meter-bar-fill" style={{ width: `${pct}%` }} />
        </div>
        <div class="voice-controls">
          <button type="button" class="btn btn-ghost" onClick={onResume}>
            <Play size={14} />
            {t("voice.resume")}
          </button>
          <button type="button" class="btn btn-ghost" onClick={onSuspend}>
            <Pause size={14} />
            {t("voice.pause")}
          </button>
        </div>
      </section>

      <section class="voice-log-section">
        <h2 class="voice-log-title">{t("voice.log.title")}</h2>
        {ttsLines.length === 0 && (
          <div class="empty-state">
            <div class="empty-state-title">{t("voice.log.empty")}</div>
          </div>
        )}
        <ul class="voice-log-list">
          {ttsLines
            .slice()
            .reverse()
            .map((line) => (
              <li key={line.id} class="voice-log-item">
                <div class="voice-log-item-header">
                  <span class="voice-log-item-text">{line.text}</span>
                  <time>{formatTime(line.ts)}</time>
                </div>
                {line.translations && Object.keys(line.translations).length > 0 && (
                  <ul class="voice-log-translations">
                    {Object.entries(line.translations).map(([lang, text]) => (
                      <li key={lang}>
                        <Languages size={12} />
                        <span class="voice-log-translation-lang">{lang}</span>
                        <span>{text}</span>
                      </li>
                    ))}
                  </ul>
                )}
              </li>
            ))}
        </ul>
      </section>
    </div>
  );
}
