// 予定 tab: editor for config.scheduler — daily-repeating chime
// announcements, each optionally carrying actions (move the avatar, run a
// command, …). The server hot-reloads on save, so there's no "apply" step;
// edits just autosave via useConfigDoc.
import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import {
  AlertTriangle,
  Bell,
  Clock,
  Download,
  FolderOpen,
  Loader2,
  Music,
  Play,
  Plus,
  RefreshCw,
  Save,
  SlidersHorizontal,
  Square,
  Trash2,
  Upload,
  Volume2,
  Zap,
} from "lucide-preact";
import { useConfigDoc } from "../hooks/useConfigDoc";
import { useI18n } from "../hooks/useI18n";
import type { Lang, MessageKey, Translate } from "../lib/i18n";
import { SaveChip } from "../components/SaveChip";
import { SoundField } from "../components/SoundField";
import {
  activateScheduleProfile,
  deleteScheduleProfile,
  exportScheduler,
  getSounds,
  importScheduler,
  listScheduleProfiles,
  revealSoundFolder,
  saveScheduleProfile,
  testAnnouncement,
  type ScheduleProfileSummary,
  type SoundFile,
} from "../lib/api";
import type {
  AnnouncementEntry,
  ScheduledActionEntry,
  ScheduledActionKind,
  SchedulerSection,
} from "../lib/config-types";
import { formatCountdown, formatNextRunLabel, nextRunAt, urgencyOf } from "../lib/schedule-time";
import type { ConfigDocument } from "../lib/types";
import { Toast, type ToastState } from "../components/Toast";
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
  sounds,
  onCommit,
  onDelete,
}: {
  t: Translate;
  action: ScheduledActionEntry;
  /** The sound library, for a `speak` action's chime picker. */
  sounds: SoundFile[];
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
            {/* Committed straight from the picker rather than through
                `draft`: a select has no blur to wait for, and routing it via
                the draft would leave the chime uncommitted until some other
                field in the row happened to blur. */}
            <SoundField
              t={t}
              label={t("schedule.field.chime")}
              value={draft.chime_file ?? ""}
              sounds={sounds}
              onCommit={(chime_file) => onCommit({ ...draft, chime_file })}
              class="schedule-action-narrow"
              showLabel={false}
            />
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
  /** `{data_dir}/sound/`'s contents, for the chime and BGM pickers. Loaded
   *  once by the view and handed down rather than fetched per row. */
  sounds: SoundFile[];
  onCommit: (patch: Partial<AnnouncementEntry>) => void;
  onCommitVolume: (volume: number) => void;
  onTest: (patch: {
    text: string;
    chime_file: string;
    actions: ScheduledActionEntry[];
    bgm_file: string;
    bgm_volume: number;
    bgm_play_full: boolean;
    bgm_end_time: string;
  }) => Promise<{ fired: boolean; spoke: boolean; actions: number }>;
  onDelete: () => void;
  /** Whether this is the row the parent currently considers "playing" — see
   *  `ScheduleView`'s `activeTestIndex`. Drives the button between
   *  テスト実行/Play and 停止/Stop; only one row is active at a time. */
  isActive: boolean;
  /** Tell the parent this row just fired a test that may produce audio. */
  onActivate: () => void;
  /** Tell the parent this row is no longer the one to show as playing
   *  (test turned out to be a no-op, errored, or the user hit stop). */
  onDeactivate: () => void;
  /** Cut the currently playing audio right now (same `{type:"interrupt"}`
   *  the チャット tab's 割り込み button sends — it stops whatever is
   *  playing through npc-speech, scheduler tests included). */
  onInterrupt: () => void;
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
  sounds,
  onCommit,
  onCommitVolume,
  onTest,
  onDelete,
  isActive,
  onActivate,
  onDeactivate,
  onInterrupt,
}: AnnouncementRowProps) {
  const [time, setTime] = useState(entry.time);
  const [text, setText] = useState(entry.text);
  const [bgmEndTime, setBgmEndTime] = useState(entry.bgm_end_time ?? "");
  // The chime/BGM pickers commit on change, so unlike the text fields above
  // they read the saved entry directly instead of keeping a draft.
  const chimeFile = entry.chime_file ?? "";
  const bgmFile = entry.bgm_file ?? "";
  const [test, setTest] = useState<TestState>({ kind: "idle" });
  const volume = entry.volume ?? 1;
  // 0/omitted means "use scheduler.defaults.bgm_volume" (see config-types.ts).
  const bgmVolume = entry.bgm_volume ?? 0;
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
    setBgmEndTime(entry.bgm_end_time ?? "");
  }, [entry.time, entry.text, entry.bgm_end_time]);

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

  /** Click while idle: fire the test and (optimistically) mark this row as
   *  the one playing. Click while `isActive`: this *is* the stop button —
   *  cut the audio right now instead of firing another test on top of it. */
  async function handleTestClick() {
    if (isActive) {
      onInterrupt();
      onDeactivate();
      return;
    }

    setTest({ kind: "running" });
    onActivate();
    try {
      const result = await onTest({
        text,
        chime_file: chimeFile,
        actions,
        bgm_file: bgmFile,
        bgm_volume: bgmVolume,
        bgm_play_full: entry.bgm_play_full ?? false,
        bgm_end_time: bgmEndTime,
      });
      if (!result.fired) {
        onDeactivate();
        finishTest("warn", t("schedule.test.empty"));
      } else if (!result.spoke) {
        // Actions fired but nothing audible — there's nothing to "stop".
        onDeactivate();
        finishTest("ok", t("schedule.test.okWithActions", { n: String(result.actions) }));
      } else if (result.actions > 0) {
        finishTest("ok", t("schedule.test.okWithActions", { n: String(result.actions) }));
      } else {
        finishTest("ok", t("schedule.test.ok"));
      }
    } catch (err) {
      onDeactivate();
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
          class={`btn btn-ghost btn-small schedule-test-btn${isActive ? " schedule-test-btn--active" : ""}`}
          onClick={() => void handleTestClick()}
          disabled={test.kind === "running" && !isActive}
          title={isActive ? t("schedule.test.stop.tooltip") : t("schedule.test.tooltip")}
        >
          {isActive ? <Square size={13} /> : <Play size={13} />}
          {isActive ? t("schedule.test.stop") : test.kind === "running" ? t("schedule.test.running") : t("schedule.test")}
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
        <SoundField
          t={t}
          label={t("schedule.field.chime")}
          value={chimeFile}
          sounds={sounds}
          onCommit={(file) => onCommit({ chime_file: file || undefined })}
        />

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

      <div class="schedule-row-bgm">
        <div class="schedule-bgm-header">
          <Music size={13} />
          <span class="schedule-field-label">BGM</span>
        </div>
        <div class="schedule-bgm-fields">
          <SoundField
            t={t}
            label={t("schedule.field.bgmFile")}
            value={bgmFile}
            sounds={sounds}
            onCommit={(file) => onCommit({ bgm_file: file || undefined })}
          />

          <label class="schedule-volume-field">
            <span class="schedule-field-label">
              <Volume2 size={13} />
              {t("schedule.field.bgmVolume")}
            </span>
            <div class="schedule-volume-row">
              <input
                type="range"
                min={0}
                max={1}
                step={0.05}
                value={bgmVolume}
                onChange={(e) => onCommit({ bgm_volume: Number((e.target as HTMLInputElement).value) })}
              />
              <span class="schedule-volume-value">{Math.round(bgmVolume * 100)}%</span>
            </div>
          </label>

          <label class="schedule-bgm-checkbox">
            <input
              type="checkbox"
              checked={entry.bgm_play_full ?? false}
              onChange={(e) => onCommit({ bgm_play_full: (e.target as HTMLInputElement).checked })}
            />
            <span>{t("schedule.field.bgmPlayFull")}</span>
          </label>

          <label class="schedule-bgm-endtime-field">
            <span class="schedule-field-label">{t("schedule.field.bgmEndTime")}</span>
            <input
              type="text"
              placeholder={t("schedule.bgmEndTime.placeholder")}
              value={bgmEndTime}
              onInput={(e) => setBgmEndTime((e.target as HTMLInputElement).value)}
              onBlur={() => onCommit({ bgm_end_time: bgmEndTime || undefined })}
            />
          </label>
        </div>
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
              sounds={sounds}
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

type SchedulerDefaults = NonNullable<SchedulerSection["defaults"]>;

/**
 * config.scheduler.defaults — fallbacks used when a per-entry chime/volume/
 * BGM-volume field is left blank or 0. Its own small form, separate from the
 * announcement list, since it applies scheduler-wide rather than to one row.
 */
function SchedulerDefaultsSection({
  t,
  defaults,
  sounds,
  onCommit,
}: {
  t: Translate;
  defaults: SchedulerDefaults | undefined;
  sounds: SoundFile[];
  onCommit: (patch: Partial<SchedulerDefaults>) => void;
}) {
  const volume = defaults?.volume ?? 1;
  const bgmVolume = defaults?.bgm_volume ?? 1;

  return (
    <section class="schedule-defaults">
      <div class="schedule-defaults-header">
        <SlidersHorizontal size={15} />
        <span>{t("schedule.defaults.title")}</span>
      </div>
      <p class="schedule-defaults-hint">{t("schedule.defaults.hint")}</p>
      <div class="schedule-defaults-fields">
        <SoundField
          t={t}
          label={t("schedule.defaults.chime")}
          value={defaults?.chime_file ?? ""}
          sounds={sounds}
          onCommit={(file) => onCommit({ chime_file: file || undefined })}
        />

        <label class="schedule-volume-field">
          <span class="schedule-field-label">
            <Volume2 size={13} />
            {t("schedule.defaults.volume")}
          </span>
          <div class="schedule-volume-row">
            <input
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={volume}
              onChange={(e) => onCommit({ volume: Number((e.target as HTMLInputElement).value) })}
            />
            <span class="schedule-volume-value">{Math.round(volume * 100)}%</span>
          </div>
        </label>

        <label class="schedule-volume-field">
          <span class="schedule-field-label">
            <Music size={13} />
            {t("schedule.defaults.bgmVolume")}
          </span>
          <div class="schedule-volume-row">
            <input
              type="range"
              min={0}
              max={1}
              step={0.05}
              value={bgmVolume}
              onChange={(e) => onCommit({ bgm_volume: Number((e.target as HTMLInputElement).value) })}
            />
            <span class="schedule-volume-value">{Math.round(bgmVolume * 100)}%</span>
          </div>
        </label>
      </div>
    </section>
  );
}

/**
 * config.scheduler profiles: named, server-saved snapshots of the whole
 * scheduler section — a persistent counterpart to the Export/Import
 * buttons' one-off JSON file above. "Save current as profile" snapshots the
 * *live* config server-side; "Switch" replaces the live config with a saved
 * snapshot, so on success it reloads the shared config doc via `onChanged`
 * rather than hand-reconstructing state locally (same reasoning as the
 * Import flow uses).
 */
function ScheduleProfilesSection({
  t,
  onChanged,
  setToast,
}: {
  t: Translate;
  onChanged: () => Promise<void>;
  setToast: (toast: ToastState | null) => void;
}) {
  const [profiles, setProfiles] = useState<ScheduleProfileSummary[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const reload = useCallback(() => {
    setLoadError(null);
    listScheduleProfiles()
      .then(setProfiles)
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  async function handleSaveAs() {
    const label = window.prompt(t("schedule.profiles.saveAs.prompt"));
    if (label === null || label.trim() === "") return;
    setSaving(true);
    try {
      await saveScheduleProfile({ id: crypto.randomUUID(), label: label.trim() });
      reload();
      setToast({ kind: "success", message: t("schedule.profiles.saveAs.ok") });
    } catch (err) {
      setToast({
        kind: "error",
        message: `${t("schedule.profiles.saveAs.error")}: ${err instanceof Error ? err.message : String(err)}`,
      });
    } finally {
      setSaving(false);
    }
  }

  async function handleActivate(profile: ScheduleProfileSummary) {
    if (!window.confirm(t("schedule.profiles.activate.confirm", { label: profile.label }))) return;
    setBusyId(profile.id);
    try {
      await activateScheduleProfile(profile.id);
      // The activation already persisted server-side and replaced the live
      // scheduler; pull the shared config doc rather than splicing the
      // response in locally, then refresh which profile shows as active.
      await onChanged();
      reload();
      setToast({ kind: "success", message: t("schedule.profiles.activate.ok") });
    } catch (err) {
      setToast({
        kind: "error",
        message: `${t("schedule.profiles.activate.error")}: ${err instanceof Error ? err.message : String(err)}`,
      });
    } finally {
      setBusyId(null);
    }
  }

  async function handleDelete(profile: ScheduleProfileSummary) {
    if (!window.confirm(t("schedule.profiles.delete.confirm", { label: profile.label }))) return;
    setBusyId(profile.id);
    try {
      await deleteScheduleProfile(profile.id);
      reload();
    } catch (err) {
      setToast({
        kind: "error",
        message: `${t("schedule.profiles.delete.error")}: ${err instanceof Error ? err.message : String(err)}`,
      });
    } finally {
      setBusyId(null);
    }
  }

  return (
    <section class="schedule-profiles">
      <div class="schedule-profiles-header">
        <FolderOpen size={15} />
        <span>{t("schedule.profiles.title")}</span>
        <button
          type="button"
          class="icon-btn schedule-profiles-refresh"
          onClick={reload}
          title={t("common.reload")}
        >
          <RefreshCw size={13} />
        </button>
        <button
          type="button"
          class="btn btn-ghost btn-small schedule-profiles-save"
          disabled={saving}
          onClick={() => void handleSaveAs()}
        >
          {saving ? <Loader2 size={14} class="spin" /> : <Save size={14} />}
          {t("schedule.profiles.saveAs")}
        </button>
      </div>
      <p class="schedule-profiles-hint">{t("schedule.profiles.hint")}</p>

      {loadError && (
        <p class="schedule-profiles-error">
          {t("schedule.profiles.loadError")}: {loadError}
        </p>
      )}

      {!profiles && !loadError && <p class="schedule-profiles-loading">{t("common.loading")}</p>}

      {profiles && profiles.length === 0 && (
        <p class="schedule-profiles-empty">{t("schedule.profiles.empty")}</p>
      )}

      {profiles && profiles.length > 0 && (
        <ul class="schedule-profiles-list">
          {profiles.map((profile) => (
            <li key={profile.id} class="schedule-profile-row">
              <span class="schedule-profile-label">{profile.label}</span>
              {profile.active ? (
                <span class="badge badge--success">{t("schedule.profiles.active.badge")}</span>
              ) : (
                <button
                  type="button"
                  class="btn btn-ghost btn-small"
                  disabled={busyId === profile.id}
                  onClick={() => void handleActivate(profile)}
                  title={t("schedule.profiles.activate.tooltip")}
                >
                  {busyId === profile.id ? <Loader2 size={13} class="spin" /> : null}
                  {t("schedule.profiles.activate")}
                </button>
              )}
              <button
                type="button"
                class="icon-btn"
                disabled={busyId === profile.id}
                onClick={() => void handleDelete(profile)}
                title={t("schedule.profiles.delete.tooltip")}
              >
                <Trash2 size={14} />
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/**
 * `{data_dir}/sound/`: what's in it, where it is, and a way to re-read it.
 *
 * Loaded once for the whole view and handed down, rather than fetched by each
 * picker — every row offers the same library, and a page with a dozen
 * announcements would otherwise issue a dozen identical requests.
 *
 * The re-read on window focus is what makes the 音声フォルダ button feel like
 * part of the app: the operator clicks it, drops files in the file manager,
 * and comes back — at which point the new files are already in the pickers,
 * with no reload and no "refresh" step to remember. A directory listing is
 * cheap enough that doing it on every focus costs nothing worth saving.
 */
function useSoundLibrary() {
  const [sounds, setSounds] = useState<SoundFile[]>([]);
  const [dir, setDir] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);

  const reload = useCallback(() => {
    getSounds()
      .then((res) => {
        setSounds(res.sounds);
        setDir(res.dir);
        setLoadError(null);
      })
      .catch((err) => setLoadError(err instanceof Error ? err.message : String(err)));
  }, []);

  useEffect(() => {
    reload();
    window.addEventListener("focus", reload);
    return () => window.removeEventListener("focus", reload);
  }, [reload]);

  return { sounds, dir, loadError, reload };
}

/** Save `data` as a downloaded JSON file — there is no shared "download a
 *  blob" helper in this codebase yet, so this is written from scratch. */
function downloadJson(data: unknown, filename: string) {
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: "application/json" });
  const url = URL.createObjectURL(blob);
  try {
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    a.click();
  } finally {
    URL.revokeObjectURL(url);
  }
}

export function ScheduleView({ speaking, onInterrupt }: { speaking: boolean; onInterrupt: () => void }) {
  const { t, lang } = useI18n();
  const { config, loadError, saveState, saveError, mutate, reload } = useConfigDoc();
  const now = useNow();
  const [toast, setToast] = useState<ToastState | null>(null);
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const importInputRef = useRef<HTMLInputElement | null>(null);
  const { sounds, dir: soundDir, loadError: soundLoadError, reload: reloadSounds } = useSoundLibrary();

  // Which row's テスト実行 button is showing 停止 right now — at most one,
  // since a scheduler test is a single global playback queue (see
  // AnnouncementRow's onTest/onInterrupt). Cleared automatically once
  // `speaking` drops from true to false (audio actually finished), not just
  // whenever it's currently false — `speaking` is false at mount/between
  // clicks too, and clearing on a plain "is false" read would race the
  // optimistic activation below.
  const [activeTestIndex, setActiveTestIndex] = useState<number | null>(null);
  const prevSpeakingRef = useRef(speaking);
  useEffect(() => {
    if (prevSpeakingRef.current && !speaking) {
      setActiveTestIndex(null);
    }
    prevSpeakingRef.current = speaking;
  }, [speaking]);

  useEffect(() => {
    if (!toast) return;
    const id = window.setTimeout(() => setToast(null), 4000);
    return () => window.clearTimeout(id);
  }, [toast]);

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

  function updateDefaults(fn: (defaults: SchedulerDefaults) => void) {
    updateScheduler((section) => {
      const current = section.defaults ?? {};
      fn(current);
      section.defaults = current;
    });
  }

  /** Hand the sound folder to the OS file manager — the browser can't, so
   *  the server does it (POST /api/sound/reveal). */
  async function handleRevealSoundFolder() {
    try {
      await revealSoundFolder();
      // Anything dropped in from here lands via the focus listener in
      // useSoundLibrary; this covers the case where the folder was already
      // open and focus never left the page.
      reloadSounds();
    } catch (err) {
      setToast({
        kind: "error",
        message: `${t("sound.folder.error")}: ${err instanceof Error ? err.message : String(err)}`,
      });
    }
  }

  async function handleExport() {
    setExporting(true);
    try {
      const data = await exportScheduler();
      const iso = new Date().toISOString().slice(0, 10);
      downloadJson(data, `tc-npc-scheduler-${iso}.json`);
    } catch (err) {
      setToast({
        kind: "error",
        message: `${t("schedule.export.error")}: ${err instanceof Error ? err.message : String(err)}`,
      });
    } finally {
      setExporting(false);
    }
  }

  async function handleImportFile(file: File) {
    if (!window.confirm(t("schedule.import.confirm"))) return;
    setImporting(true);
    try {
      const text = await file.text();
      const data = JSON.parse(text) as SchedulerSection;
      await importScheduler(data);
      await reload();
      setToast({ kind: "success", message: t("schedule.import.ok") });
    } catch (err) {
      setToast({
        kind: "error",
        message: `${t("schedule.import.error")}: ${err instanceof Error ? err.message : String(err)}`,
      });
    } finally {
      setImporting(false);
    }
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
        <div class="schedule-header-actions">
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

          <button
            type="button"
            class="btn btn-ghost btn-small"
            onClick={() => void handleRevealSoundFolder()}
            title={t("sound.folder.tooltip")}
          >
            <FolderOpen size={14} />
            {t("sound.folder")}
          </button>

          <button
            type="button"
            class="btn btn-ghost btn-small"
            disabled={exporting}
            onClick={() => void handleExport()}
            title={t("schedule.export.tooltip")}
          >
            {exporting ? <Loader2 size={14} class="spin" /> : <Download size={14} />}
            {t("schedule.export")}
          </button>

          <button
            type="button"
            class="btn btn-ghost btn-small"
            disabled={importing}
            onClick={() => importInputRef.current?.click()}
            title={t("schedule.import.tooltip")}
          >
            {importing ? <Loader2 size={14} class="spin" /> : <Upload size={14} />}
            {t("schedule.import")}
          </button>
          <input
            ref={importInputRef}
            type="file"
            accept="application/json,.json"
            class="visually-hidden"
            onChange={(e) => {
              const file = (e.target as HTMLInputElement).files?.[0];
              if (file) void handleImportFile(file);
              (e.target as HTMLInputElement).value = "";
            }}
          />
        </div>
      </div>

      <p class="schedule-hint">{t("schedule.hint")}</p>

      {/* Where audio goes. Spelled out rather than left to the tooltip on the
          button above: "which folder do chimes live in" is the first question
          this tab raises, and the answer is a path only the server knows. */}
      {/* Held back until the listing resolves: the sentence is about a path,
          and rendering it with an empty one reads as "put your files in ". */}
      {(soundLoadError !== null || soundDir !== "") && (
        <p class="schedule-hint schedule-sound-hint">
          {soundLoadError !== null
            ? `${t("sound.loadError")}: ${soundLoadError}`
            : t("sound.folder.hint", { dir: soundDir })}
          {soundLoadError === null && sounds.length === 0 && ` — ${t("sound.empty")}`}
        </p>
      )}

      <ScheduleProfilesSection t={t} onChanged={reload} setToast={setToast} />

      <SchedulerDefaultsSection
        t={t}
        defaults={scheduler.defaults}
        sounds={sounds}
        onCommit={(patch) => updateDefaults((d) => Object.assign(d, patch))}
      />

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
              sounds={sounds}
              onTest={(patch) => testAnnouncement({ index, ...patch })}
              isActive={activeTestIndex === index}
              onActivate={() => setActiveTestIndex(index)}
              onDeactivate={() => setActiveTestIndex((current) => (current === index ? null : current))}
              onInterrupt={onInterrupt}
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

      {toast && <Toast toast={toast} />}
    </div>
  );
}
