// 予定 tab: editor for config.scheduler — daily-repeating chime
// announcements, each optionally carrying actions (move the avatar, run a
// command, …). The server hot-reloads on save, so there's no "apply" step;
// edits just autosave via useConfigDoc.
import { useEffect, useRef, useState } from "preact/hooks";
import { AlertTriangle, Bell, Clock, Play, Plus, Trash2, Volume2, Zap } from "lucide-preact";
import { useConfigDoc } from "../hooks/useConfigDoc";
import { useI18n } from "../hooks/useI18n";
import type { Lang, MessageKey, Translate } from "../lib/i18n";
import { SaveChip } from "../components/SaveChip";
import { testAnnouncement } from "../lib/api";
import type {
  AnnouncementEntry,
  ScheduledActionEntry,
  ScheduledActionKind,
  SchedulerSection,
} from "../lib/config-types";
import { formatCountdown, formatNextRunLabel, nextRunAt, urgencyOf } from "../lib/schedule-time";
import type { ConfigDocument } from "../lib/types";
import "../styles/components.css";
import "../styles/schedule.css";

/** How long a テスト実行 result chip stays up before clearing itself. */
const TEST_RESULT_MS = 4000;

function readScheduler(config: ConfigDocument): SchedulerSection {
  return (config.scheduler as SchedulerSection | undefined) ?? {};
}

/** Re-renders once a second so every countdown ticks off the same clock. */
function useNow(): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 1000);
    return () => clearInterval(id);
  }, []);
  return now;
}

type TestState = { kind: "idle" } | { kind: "running" } | { kind: "done"; tone: "ok" | "warn" | "error"; message: string };

/** Selectable action kinds, in the order they appear in the dropdown. */
const ACTION_KINDS: ScheduledActionKind[] = ["action", "command", "chat", "speak", "suspend", "resume", "raw"];

const ACTION_KIND_LABELS: Record<ScheduledActionKind, MessageKey> = {
  speak: "schedule.action.kind.speak",
  action: "schedule.action.kind.action",
  command: "schedule.action.kind.command",
  chat: "schedule.action.kind.chat",
  suspend: "schedule.action.kind.suspend",
  resume: "schedule.action.kind.resume",
  raw: "schedule.action.kind.raw",
};

/** A blank entry of `kind`, used when adding a row or switching its kind. */
function defaultAction(kind: ScheduledActionKind): ScheduledActionEntry {
  switch (kind) {
    case "speak":
      return { kind: "speak", content: "", chime_file: "" };
    case "action":
      return { kind: "action", content: "" };
    case "command":
      return { kind: "command", text: "" };
    case "chat":
      return { kind: "chat", content: "" };
    case "suspend":
      return { kind: "suspend" };
    case "resume":
      return { kind: "resume" };
    case "raw":
      return { kind: "raw", topic: "agent:interrupt", type: "resume", payload: {} };
  }
}

/**
 * One action row. Like the announcement fields above, every input edits a
 * local draft that only flows back into the config on blur — the parent
 * re-renders once a second for the countdowns, which would otherwise yank
 * half-typed text back to the saved value.
 *
 * `raw`'s payload is edited as JSON text and has to parse before it can be
 * committed; an invalid draft shows an error and leaves the saved value alone.
 */
function ActionRow({
  t,
  action,
  onCommit,
  onDelete,
}: {
  t: Translate;
  action: ScheduledActionEntry;
  onCommit: (next: ScheduledActionEntry) => void;
  onDelete: () => void;
}) {
  const [draft, setDraft] = useState<ScheduledActionEntry>(action);
  const payloadText = action.kind === "raw" ? JSON.stringify(action.payload ?? {}) : "";
  const [payloadDraft, setPayloadDraft] = useState(payloadText);
  const [payloadError, setPayloadError] = useState(false);

  // Re-sync when the committed action actually changes (kind switch, or a
  // delete shifting a later row onto this component). Typing doesn't change
  // it, so the caret is safe.
  const serialized = JSON.stringify(action);
  useEffect(() => {
    setDraft(action);
    setPayloadDraft(action.kind === "raw" ? JSON.stringify(action.payload ?? {}) : "");
    setPayloadError(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serialized]);

  /** Update the draft in place; the value only reaches the config on blur. */
  function edit(patch: Record<string, string>) {
    setDraft((current) => ({ ...current, ...patch }) as ScheduledActionEntry);
  }

  function commitPayload() {
    if (draft.kind !== "raw") return;
    try {
      const parsed = JSON.parse(payloadDraft.trim() === "" ? "{}" : payloadDraft) as unknown;
      setPayloadError(false);
      onCommit({ ...draft, payload: parsed });
    } catch {
      setPayloadError(true);
    }
  }

  function textInput(
    value: string,
    key: string,
    placeholder: string,
    extraClass?: string,
  ) {
    return (
      <input
        type="text"
        class={extraClass}
        placeholder={placeholder}
        value={value}
        onInput={(e) => edit({ [key]: (e.target as HTMLInputElement).value })}
        onBlur={() => onCommit(draft)}
      />
    );
  }

  return (
    <div class="schedule-action">
      <select
        class="schedule-action-kind"
        value={draft.kind}
        onChange={(e) => onCommit(defaultAction((e.target as HTMLSelectElement).value as ScheduledActionKind))}
        title={t("schedule.action.kind")}
      >
        {ACTION_KINDS.map((kind) => (
          <option key={kind} value={kind}>
            {t(ACTION_KIND_LABELS[kind])}
          </option>
        ))}
      </select>

      <div class="schedule-action-fields">
        {draft.kind === "action" && textInput(draft.content ?? "", "content", t("schedule.action.placeholder.action"))}
        {draft.kind === "command" && textInput(draft.text ?? "", "text", t("schedule.action.placeholder.command"))}
        {draft.kind === "chat" && textInput(draft.content ?? "", "content", t("schedule.action.placeholder.chat"))}
        {draft.kind === "speak" && (
          <>
            {textInput(draft.content ?? "", "content", t("schedule.text.placeholder"))}
            {textInput(draft.chime_file ?? "", "chime_file", t("schedule.chime.placeholder"), "schedule-action-narrow")}
          </>
        )}
        {draft.kind === "raw" && (
          <>
            {textInput(draft.topic, "topic", t("schedule.action.field.topic"), "schedule-action-narrow")}
            {textInput(draft.type, "type", t("schedule.action.field.type"), "schedule-action-narrow")}
            <input
              type="text"
              placeholder={t("schedule.action.field.payload")}
              value={payloadDraft}
              onInput={(e) => setPayloadDraft((e.target as HTMLInputElement).value)}
              onBlur={commitPayload}
            />
            {payloadError && <span class="schedule-action-error">{t("schedule.action.invalidJson")}</span>}
          </>
        )}
        {(draft.kind === "suspend" || draft.kind === "resume") && (
          <span class="schedule-action-note">{t("schedule.action.noFields")}</span>
        )}
      </div>

      <button type="button" class="icon-btn" onClick={onDelete} title={t("schedule.action.delete.tooltip")}>
        <Trash2 size={14} />
      </button>
    </div>
  );
}

interface AnnouncementRowProps {
  t: Translate;
  lang: Lang;
  entry: AnnouncementEntry;
  now: Date;
  /** Grey out the countdown when the whole scheduler is switched off. */
  schedulerEnabled: boolean;
  onCommit: (patch: Partial<AnnouncementEntry>) => void;
  onCommitVolume: (volume: number) => void;
  onTest: (patch: {
    text: string;
    chime_file: string;
    actions: ScheduledActionEntry[];
  }) => Promise<{ fired: boolean; spoke: boolean; actions: number }>;
  onDelete: () => void;
}

/**
 * "あと 2時間13分4秒 · 明日 09:00:00" for one announcement, computed from the
 * row's draft time so it updates as the user edits.
 */
function CountdownChip({
  t,
  lang,
  time,
  now,
  enabled,
}: {
  t: Translate;
  lang: Lang;
  time: string;
  now: Date;
  enabled: boolean;
}) {
  const next = nextRunAt(time, now);
  if (next === null) {
    return (
      <span class="schedule-countdown schedule-countdown--invalid">
        <AlertTriangle size={13} />
        {t("schedule.invalidTime")}
      </span>
    );
  }

  const msUntil = next.getTime() - now.getTime();
  const urgency = enabled ? urgencyOf(msUntil) : "disabled";

  return (
    <span class={`schedule-countdown schedule-countdown--${urgency}`}>
      <Clock size={13} />
      {t("schedule.remaining", { duration: formatCountdown(msUntil, lang) })}
      <span class="schedule-countdown-at">{formatNextRunLabel(next, now, lang)}</span>
      {!enabled && <span class="schedule-countdown-note">{t("schedule.disabledNote")}</span>}
    </span>
  );
}

// A single announcement row. Time / text / chime path use a local draft
// that only flows back into the config on blur, so a debounce-triggered
// re-render never yanks the caret mid-keystroke. The volume slider has no
// such risk (native range drag doesn't fight React re-renders the same
// way), so it commits on every change.
function AnnouncementRow({
  t,
  lang,
  entry,
  now,
  schedulerEnabled,
  onCommit,
  onCommitVolume,
  onTest,
  onDelete,
}: AnnouncementRowProps) {
  const [time, setTime] = useState(entry.time);
  const [text, setText] = useState(entry.text);
  const [chimeFile, setChimeFile] = useState(entry.chime_file ?? "");
  const [test, setTest] = useState<TestState>({ kind: "idle" });
  const volume = entry.volume ?? 1;
  const actions = entry.actions ?? [];

  function updateActions(fn: (list: ScheduledActionEntry[]) => void) {
    const next = [...actions];
    fn(next);
    onCommit({ actions: next });
  }

  // Rows are keyed by index, so deleting one shifts every later entry onto
  // an already-mounted component whose drafts still hold the previous
  // entry's values. Re-sync whenever the committed values actually change —
  // typing doesn't (the commit only happens on blur, and then the draft
  // already equals what arrives here), so the caret is still safe.
  useEffect(() => {
    setTime(entry.time);
    setText(entry.text);
    setChimeFile(entry.chime_file ?? "");
  }, [entry.time, entry.text, entry.chime_file]);

  // The result chip clears itself; the timer lives in a ref so a second test
  // started before the first chip expires resets the countdown rather than
  // being wiped by the earlier timer.
  const resultTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => {
    if (resultTimer.current !== null) clearTimeout(resultTimer.current);
  }, []);

  function finishTest(tone: "ok" | "warn" | "error", message: string) {
    setTest({ kind: "done", tone, message });
    if (resultTimer.current !== null) clearTimeout(resultTimer.current);
    resultTimer.current = setTimeout(() => setTest({ kind: "idle" }), TEST_RESULT_MS);
  }

  async function runTest() {
    setTest({ kind: "running" });
    try {
      const result = await onTest({ text, chime_file: chimeFile, actions });
      if (!result.fired) {
        finishTest("warn", t("schedule.test.empty"));
      } else if (result.actions > 0) {
        finishTest("ok", t("schedule.test.okWithActions", { n: String(result.actions) }));
      } else {
        finishTest("ok", t("schedule.test.ok"));
      }
    } catch (err) {
      finishTest("error", err instanceof Error ? err.message : String(err));
    }
  }

  return (
    <div class="schedule-row">
      <div class="schedule-row-status">
        <CountdownChip t={t} lang={lang} time={time} now={now} enabled={schedulerEnabled} />
        {test.kind === "done" && (
          <span class={`schedule-test-result schedule-test-result--${test.tone}`}>{test.message}</span>
        )}
      </div>

      <div class="schedule-row-main">
        <label class="schedule-time-field">
          <span class="schedule-field-label">{t("schedule.field.time")}</span>
          <input
            type="time"
            value={time}
            onInput={(e) => setTime((e.target as HTMLInputElement).value)}
            onBlur={() => onCommit({ time })}
          />
        </label>

        <label class="schedule-text-field">
          <span class="schedule-field-label">{t("schedule.field.text")}</span>
          <input
            type="text"
            placeholder={t("schedule.text.placeholder")}
            value={text}
            onInput={(e) => setText((e.target as HTMLInputElement).value)}
            onBlur={() => onCommit({ text })}
          />
        </label>

        <button
          type="button"
          class="btn btn-ghost btn-small schedule-test-btn"
          onClick={() => void runTest()}
          disabled={test.kind === "running"}
          title={t("schedule.test.tooltip")}
        >
          <Play size={13} />
          {test.kind === "running" ? t("schedule.test.running") : t("schedule.test")}
        </button>

        <button
          type="button"
          class="icon-btn schedule-delete-btn"
          onClick={onDelete}
          title={t("schedule.delete.tooltip")}
        >
          <Trash2 size={16} />
        </button>
      </div>

      <div class="schedule-row-sub">
        <label class="schedule-chime-field">
          <span class="schedule-field-label">{t("schedule.field.chime")}</span>
          <input
            type="text"
            placeholder={t("schedule.chime.placeholder")}
            value={chimeFile}
            onInput={(e) => setChimeFile((e.target as HTMLInputElement).value)}
            onBlur={() => onCommit({ chime_file: chimeFile || undefined })}
          />
        </label>

        <label class="schedule-volume-field">
          <span class="schedule-field-label">
            <Volume2 size={13} />
            {t("schedule.field.volume")}
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

      <div class="schedule-row-actions">
        <div class="schedule-actions-header">
          <Zap size={13} />
          <span class="schedule-field-label">{t("schedule.actions.title")}</span>
        </div>
        {actions.length === 0 ? (
          <p class="schedule-actions-empty">{t("schedule.actions.empty")}</p>
        ) : (
          actions.map((action, index) => (
            <ActionRow
              key={index}
              t={t}
              action={action}
              onCommit={(next) => updateActions((list) => void (list[index] = next))}
              onDelete={() => updateActions((list) => void list.splice(index, 1))}
            />
          ))
        )}
        <button
          type="button"
          class="btn btn-ghost btn-small"
          onClick={() => updateActions((list) => void list.push(defaultAction("action")))}
        >
          <Plus size={13} />
          {t("schedule.actions.add")}
        </button>
      </div>
    </div>
  );
}

/**
 * "次の実行: おはよう — あと 2時間13分4秒（今日 09:00:00）", i.e. the soonest
 * valid announcement across the whole list — the web equivalent of the Go
 * TUI's sorted announcement queue.
 */
function NextUpSummary({
  t,
  lang,
  announcements,
  now,
  enabled,
}: {
  t: Translate;
  lang: Lang;
  announcements: AnnouncementEntry[];
  now: Date;
  enabled: boolean;
}) {
  const upcoming = announcements
    .map((entry) => ({ entry, next: nextRunAt(entry.time, now) }))
    .filter((item): item is { entry: AnnouncementEntry; next: Date } => item.next !== null)
    .sort((a, b) => a.next.getTime() - b.next.getTime());

  if (upcoming.length === 0) return null;

  const { entry, next } = upcoming[0];
  const msUntil = next.getTime() - now.getTime();

  return (
    <div class={`schedule-next-up${enabled ? "" : " schedule-next-up--disabled"}`}>
      <Clock size={15} />
      <span class="schedule-next-up-label">{t("schedule.nextUp")}</span>
      <strong class="schedule-next-up-countdown">
        {t("schedule.remaining", { duration: formatCountdown(msUntil, lang) })}
      </strong>
      <span class="schedule-next-up-at">{formatNextRunLabel(next, now, lang)}</span>
      {entry.text !== "" && <span class="schedule-next-up-text">{entry.text}</span>}
    </div>
  );
}

export function ScheduleView() {
  const { t, lang } = useI18n();
  const { config, loadError, saveState, saveError, mutate } = useConfigDoc();
  const now = useNow();

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
        <div class="empty-state-title">{t("schedule.loadError")}</div>
        <div class="empty-state-description">{loadError}</div>
      </div>
    );
  }

  if (!config) {
    return <div class="empty-state">{t("common.loading")}</div>;
  }

  const scheduler = readScheduler(config);
  const announcements = scheduler.announcements ?? [];

  return (
    <div class="schedule-view">
      <div class="schedule-header">
        <div class="schedule-header-title">
          <Bell size={18} />
          <h2>{t("schedule.title")}</h2>
          <SaveChip state={saveState} error={saveError} />
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
          <span>{t("schedule.enable")}</span>
        </label>
      </div>

      <p class="schedule-hint">{t("schedule.hint")}</p>

      <NextUpSummary
        t={t}
        lang={lang}
        announcements={announcements}
        now={now}
        enabled={scheduler.enabled ?? false}
      />

      {announcements.length === 0 ? (
        <div class="empty-state">
          <div class="empty-state-icon">
            <Bell size={24} />
          </div>
          <div class="empty-state-title">{t("schedule.empty.title")}</div>
          <div class="empty-state-description">{t("schedule.empty.desc")}</div>
        </div>
      ) : (
        <div class="schedule-list">
          {announcements.map((entry, index) => (
            <AnnouncementRow
              key={index}
              t={t}
              lang={lang}
              entry={entry}
              now={now}
              schedulerEnabled={scheduler.enabled ?? false}
              onTest={(patch) => testAnnouncement({ index, ...patch })}
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
        {t("schedule.add")}
      </button>
    </div>
  );
}
