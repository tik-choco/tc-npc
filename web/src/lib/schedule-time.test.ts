// Unit tests for the 予定 view's countdown helpers, which deliberately mirror
// crates/npc-scheduler/src/lib.rs's parse_time/next_occurrence — so the
// interesting cases here are the same ones that module's own tests should
// agree with: single-digit HH:MM(:SS), out-of-range components, "now" itself
// counting as already passed, and the three languages' unit templates.
import { describe, expect, it } from "vitest";
import {
  formatClock,
  formatCountdown,
  formatNextRunLabel,
  isNextDay,
  nextRunAt,
  urgencyOf,
} from "./schedule-time";

describe("nextRunAt", () => {
  it("returns today's occurrence when the time hasn't passed yet", () => {
    const now = new Date(2024, 0, 1, 8, 0, 0, 0);
    const at = nextRunAt("09:00", now);
    expect(at).not.toBeNull();
    expect(at!.getDate()).toBe(1);
    expect(at!.getHours()).toBe(9);
    expect(at!.getMinutes()).toBe(0);
  });

  it("rolls over to tomorrow once the time has already passed today", () => {
    const now = new Date(2024, 0, 1, 10, 0, 0, 0);
    const at = nextRunAt("09:00", now);
    expect(at!.getDate()).toBe(2);
    expect(at!.getHours()).toBe(9);
  });

  it("treats a time exactly equal to now as already passed (fires tomorrow, not immediately)", () => {
    const now = new Date(2024, 0, 1, 9, 0, 0, 0);
    const at = nextRunAt("09:00:00", now);
    expect(at!.getDate()).toBe(2);
  });

  it("accepts single-digit hour/minute/second fields, matching the Rust parser", () => {
    const now = new Date(2024, 0, 1, 0, 0, 0, 0);
    const at = nextRunAt("9:5:3", now);
    expect(at).not.toBeNull();
    expect(at!.getHours()).toBe(9);
    expect(at!.getMinutes()).toBe(5);
    expect(at!.getSeconds()).toBe(3);
  });

  it("defaults seconds to 0 when only HH:MM is given", () => {
    const now = new Date(2024, 0, 1, 0, 0, 0, 0);
    const at = nextRunAt("09:00", now);
    expect(at!.getSeconds()).toBe(0);
  });

  it("trims surrounding whitespace", () => {
    const now = new Date(2024, 0, 1, 0, 0, 0, 0);
    expect(nextRunAt("  09:00  ", now)).not.toBeNull();
  });

  it("rejects an out-of-range hour, minute, or second", () => {
    const now = new Date(2024, 0, 1, 0, 0, 0, 0);
    expect(nextRunAt("24:00", now)).toBeNull();
    expect(nextRunAt("12:60", now)).toBeNull();
    expect(nextRunAt("12:30:60", now)).toBeNull();
  });

  it("rejects a string that isn't HH:MM or HH:MM:SS", () => {
    const now = new Date(2024, 0, 1, 0, 0, 0, 0);
    expect(nextRunAt("not a time", now)).toBeNull();
    expect(nextRunAt("9", now)).toBeNull();
    expect(nextRunAt("", now)).toBeNull();
  });
});

describe("urgencyOf", () => {
  it("is imminent just under one minute out", () => {
    expect(urgencyOf(59_000)).toBe("imminent");
    expect(urgencyOf(0)).toBe("imminent");
  });

  it("is soon at exactly one minute out (the imminent boundary is exclusive)", () => {
    expect(urgencyOf(60_000)).toBe("soon");
  });

  it("is soon just under five minutes out", () => {
    expect(urgencyOf(5 * 60_000 - 1)).toBe("soon");
  });

  it("is scheduled at exactly five minutes out (the soon boundary is exclusive)", () => {
    expect(urgencyOf(5 * 60_000)).toBe("scheduled");
  });

  it("is scheduled well beyond five minutes", () => {
    expect(urgencyOf(60 * 60_000)).toBe("scheduled");
  });
});

describe("formatCountdown", () => {
  it("renders hours/minutes/seconds tiers only down to the first non-zero tier", () => {
    expect(formatCountdown(0, "ja")).toBe("0秒");
    expect(formatCountdown(125_000, "ja")).toBe("2分5秒"); // no hours tier
    expect(formatCountdown(7_984_000, "ja")).toBe("2時間13分4秒"); // 2h13m4s
  });

  it("floors a negative countdown to zero instead of going negative", () => {
    expect(formatCountdown(-500, "ja")).toBe("0秒");
  });

  it("joins English tiers with the unit template's own trailing space", () => {
    expect(formatCountdown(7_984_000, "en")).toBe("2h 13m 4s");
    expect(formatCountdown(125_000, "en")).toBe("2m 5s");
  });

  it("uses the Chinese unit labels with no separator, like Japanese", () => {
    expect(formatCountdown(7_984_000, "zh")).toBe("2小时13分4秒");
  });
});

describe("formatClock", () => {
  it("pads single-digit hour/minute/second fields to two digits", () => {
    expect(formatClock(new Date(2024, 0, 1, 9, 5, 3))).toBe("09:05:03");
  });

  it("renders midnight as 00:00:00", () => {
    expect(formatClock(new Date(2024, 0, 1, 0, 0, 0))).toBe("00:00:00");
  });
});

describe("isNextDay", () => {
  it("is false when both dates fall on the same calendar day", () => {
    const now = new Date(2024, 0, 1, 8, 0, 0);
    const at = new Date(2024, 0, 1, 20, 0, 0);
    expect(isNextDay(at, now)).toBe(false);
  });

  it("is true once the calendar day has advanced", () => {
    const now = new Date(2024, 0, 1, 23, 0, 0);
    const at = new Date(2024, 0, 2, 1, 0, 0);
    expect(isNextDay(at, now)).toBe(true);
  });

  it("is true across a year boundary", () => {
    const now = new Date(2024, 11, 31, 23, 0, 0);
    const at = new Date(2025, 0, 1, 1, 0, 0);
    expect(isNextDay(at, now)).toBe(true);
  });
});

describe("formatNextRunLabel", () => {
  it("labels a same-day run as today, in the active language", () => {
    const now = new Date(2024, 0, 1, 8, 0, 0);
    const at = new Date(2024, 0, 1, 9, 0, 0);
    expect(formatNextRunLabel(at, now, "ja")).toBe("今日 09:00:00");
    expect(formatNextRunLabel(at, now, "en")).toBe("today 09:00:00");
  });

  it("labels a rolled-over run as tomorrow", () => {
    const now = new Date(2024, 0, 1, 10, 0, 0);
    const at = new Date(2024, 0, 2, 9, 0, 0);
    expect(formatNextRunLabel(at, now, "ja")).toBe("明日 09:00:00");
    expect(formatNextRunLabel(at, now, "zh")).toBe("明天 09:00:00");
  });
});
