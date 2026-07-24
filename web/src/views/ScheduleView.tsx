// 予定 tab: editor for config.scheduler — daily-repeating chime
// announcements. The server hot-reloads on save, so there's no "apply"
// step; edits just autosave via useConfigDoc.
import { useState } from "preact/hooks";
import { AlertTriangle, Bell, Plus, Trash2, Volume2 } from "lucide-preact";
import { useConfigDoc } from "../hooks/useConfigDoc";
import type { AnnouncementEntry, SchedulerSection } from "../lib/config-types";
import type { ConfigDocument } from "../lib/types";
import "../styles/components.css";
import "../styles/schedule.css";

function readScheduler(config: ConfigDocument): SchedulerSection {
  return (config.scheduler as SchedulerSection | undefined) ?? {};
}

interface AnnouncementRowProps {
  entry: AnnouncementEntry;
  onCommit: (patch: Partial<AnnouncementEntry>) => void;
  onCommitVolume: (volume: number) => void;
  onDelete: () => void;
}

// A single announcement row. Time / text / chime path use a local draft
// that only flows back into the config on blur, so a debounce-triggered
// re-render never yanks the caret mid-keystroke. The volume slider has no
// such risk (native range drag doesn't fight React re-renders the same
// way), so it commits on every change.
function AnnouncementRow({ entry, onCommit, onCommitVolume, onDelete }: AnnouncementRowProps) {
  const [time, setTime] = useState(entry.time);
  const [text, setText] = useState(entry.text);
  const [chimeFile, setChimeFile] = useState(entry.chime_file ?? "");
  const volume = entry.volume ?? 1;

  return (
    <div class="schedule-row">
      <div class="schedule-row-main">
        <label class="schedule-time-field">
          <span class="schedule-field-label">時刻</span>
          <input
            type="time"
            value={time}
            onInput={(e) => setTime((e.target as HTMLInputElement).value)}
            onBlur={() => onCommit({ time })}
          />
        </label>

        <label class="schedule-text-field">
          <span class="schedule-field-label">本文</span>
          <input
            type="text"
            placeholder="アナウンスするテキスト"
            value={text}
            onInput={(e) => setText((e.target as HTMLInputElement).value)}
            onBlur={() => onCommit({ text })}
          />
        </label>

        <button type="button" class="icon-btn schedule-delete-btn" onClick={onDelete} title="この予定を削除">
          <Trash2 size={16} />
        </button>
      </div>

      <div class="schedule-row-sub">
        <label class="schedule-chime-field">
          <span class="schedule-field-label">チャイム音ファイル（任意）</span>
          <input
            type="text"
            placeholder="例: assets/chime.wav"
            value={chimeFile}
            onInput={(e) => setChimeFile((e.target as HTMLInputElement).value)}
            onBlur={() => onCommit({ chime_file: chimeFile || undefined })}
          />
        </label>

        <label class="schedule-volume-field">
          <span class="schedule-field-label">
            <Volume2 size={13} />
            音量
          </span>
          <div class="schedule-volume-row">
            <input
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={volume}
              onChange={(e) => onCommitVolume(Number((e.target as HTMLInputElement).value))}
            />
            <span class="schedule-volume-value">{Math.round(volume * 100)}%</span>
          </div>
        </label>
      </div>
    </div>
  );
}

export function ScheduleView() {
  const { config, loadError, saveState, saveError, mutate } = useConfigDoc();

  function updateScheduler(fn: (section: SchedulerSection) => void) {
    mutate((draft) => {
      const section = ((draft as ConfigDocument).scheduler as SchedulerSection | undefined) ?? {};
      fn(section);
      (draft as ConfigDocument).scheduler = section;
    });
  }

  function updateAnnouncements(fn: (list: AnnouncementEntry[]) => void) {
    updateScheduler((section) => {
      const list = section.announcements ?? [];
      fn(list);
      section.announcements = list;
    });
  }

  if (loadError) {
    return (
      <div class="empty-state">
        <div class="empty-state-icon">
          <AlertTriangle size={24} />
        </div>
        <div class="empty-state-title">設定の読み込みに失敗しました</div>
        <div class="empty-state-description">{loadError}</div>
      </div>
    );
  }

  if (!config) {
    return <div class="empty-state">読み込み中…</div>;
  }

  const scheduler = readScheduler(config);
  const announcements = scheduler.announcements ?? [];

  return (
    <div class="schedule-view">
      <div class="schedule-header">
        <div class="schedule-header-title">
          <Bell size={18} />
          <h2>予定</h2>
          {saveState === "saving" && <span class="chip schedule-save-chip">保存中…</span>}
          {saveState === "saved" && <span class="chip schedule-save-chip schedule-save-chip--saved">保存済み</span>}
          {saveState === "error" && (
            <span class="chip schedule-save-chip schedule-save-chip--error">
              保存に失敗しました{saveError ? `: ${saveError}` : ""}
            </span>
          )}
        </div>
        <label class="schedule-enabled-toggle">
          <input
            type="checkbox"
            checked={scheduler.enabled ?? false}
            onChange={(e) =>
              updateScheduler((section) => {
                section.enabled = (e.target as HTMLInputElement).checked;
              })
            }
          />
          <span>スケジューラを有効化</span>
        </label>
      </div>

      <p class="schedule-hint">変更は自動保存され、すぐに反映されます。</p>

      {announcements.length === 0 ? (
        <div class="empty-state">
          <div class="empty-state-icon">
            <Bell size={24} />
          </div>
          <div class="empty-state-title">予定はまだありません</div>
          <div class="empty-state-description">毎日繰り返しのアナウンス予定を追加できます。</div>
        </div>
      ) : (
        <div class="schedule-list">
          {announcements.map((entry, index) => (
            <AnnouncementRow
              key={index}
              entry={entry}
              onCommit={(patch) =>
                updateAnnouncements((list) => {
                  list[index] = { ...list[index], ...patch };
                })
              }
              onCommitVolume={(volume) =>
                updateAnnouncements((list) => {
                  list[index] = { ...list[index], volume };
                })
              }
              onDelete={() =>
                updateAnnouncements((list) => {
                  list.splice(index, 1);
                })
              }
            />
          ))}
        </div>
      )}

      <button
        type="button"
        class="btn btn-ghost schedule-add-btn"
        onClick={() =>
          updateAnnouncements((list) => {
            list.push({ time: "09:00", text: "" });
          })
        }
      >
        <Plus size={14} />
        予定を追加
      </button>
    </div>
  );
}
