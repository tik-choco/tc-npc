// Countdown helpers for the 予定 view. These deliberately mirror
// crates/npc-scheduler/src/lib.rs (`parse_time` / `next_occurrence`): a
// "HH:MM" or "HH:MM:SS" local time that repeats daily, firing today if it
// hasn't passed yet and tomorrow otherwise (a time exactly equal to "now"
// counts as passed, so it doesn't re-fire immediately). Computing this in
// the browser rather than asking the server keeps the countdown live while
// the user is still typing a time that hasn't been autosaved yet.
//
// The formatters take the active UI language, since "2時間13分4秒" /
// "2h 13m 4s" / "2小时13分4秒" differ in more than wording — English needs
// spaces between the tiers, CJK doesn't.

import { translate, type Lang } from "./i18n";

// Single-digit fields are allowed because the Rust side parses each
// component with `str::parse` — a hand-edited config.json with "9:5" is
// accepted there, so the UI must not flag it as malformed.
const TIME_PATTERN = /^(\d{1,2}):(\d{1,2})(?::(\d{1,2}))?$/;

/** Under this many ms the announcement is about to fire. */
const IMMINENT_MS = 60_000;
/** Under this many ms it's coming up soon. */
const SOON_MS = 5 * 60_000;

export type Urgency = "imminent" | "soon" | "scheduled";

/**
 * The next local `Date` at which `time` occurs, or null if `time` isn't a
 * valid "HH:MM" / "HH:MM:SS" string (the server skips those entries too).
 */
export function nextRunAt(time: string, now: Date): Date | null {
  const match = TIME_PATTERN.exec(time.trim());
  if (match === null) return null;

  const hours = Number(match[1]);
  const minutes = Number(match[2]);
  const seconds = match[3] === undefined ? 0 : Number(match[3]);
  if (hours > 23 || minutes > 59 || seconds > 59) return null;

  const next = new Date(now);
  next.setHours(hours, minutes, seconds, 0);
  if (next.getTime() <= now.getTime()) next.setDate(next.getDate() + 1);
  return next;
}

export function urgencyOf(msUntil: number): Urgency {
  if (msUntil < IMMINENT_MS) return "imminent";
  if (msUntil < SOON_MS) return "soon";
  return "scheduled";
}

/** "2時間13分4秒" / "13分4秒" / "4秒" — the same h/m/s tiers the Go TUI uses. */
export function formatCountdown(msUntil: number, lang: Lang): string {
  const total = Math.max(0, Math.round(msUntil / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;

  // The unit templates carry their own separator (English's trailing space,
  // CJK's none), so the tiers below just concatenate.
  const h = translate(lang, "schedule.unit.hours", { n: hours });
  const m = translate(lang, "schedule.unit.minutes", { n: minutes });
  const s = translate(lang, "schedule.unit.seconds", { n: seconds });

  if (hours > 0) return `${h}${m}${s}`;
  if (minutes > 0) return `${m}${s}`;
  return s;
}

/** "09:00:00" — the wall-clock time the announcement will fire at. */
export function formatClock(at: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}`;
}

/** Whether `at` falls on the calendar day after `now` (i.e. already passed today). */
export function isNextDay(at: Date, now: Date): boolean {
  return at.getDate() !== now.getDate() || at.getMonth() !== now.getMonth() || at.getFullYear() !== now.getFullYear();
}

/** "今日 09:00:00" / "明日 09:00:00" — how the fire time is labelled in the UI. */
export function formatNextRunLabel(at: Date, now: Date, lang: Lang): string {
  const day = translate(lang, isNextDay(at, now) ? "schedule.tomorrow" : "schedule.today");
  return `${day} ${formatClock(at)}`;
}
