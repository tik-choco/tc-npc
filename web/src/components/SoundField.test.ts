// The picker's option list. What matters here is that switching the
// chime/BGM fields from free-text paths to a select can't lose a value: the
// sound folder is new, but `chime_file`/`bgm_file` are not, and every config
// already out there holds a path rather than a library file name.
import { describe, expect, it } from "vitest";
import { soundOptions } from "./SoundField";
import type { SoundFile } from "../lib/api";

const SOUNDS: SoundFile[] = [
  { file: "chime.wav", name: "chime", size: 1 },
  { file: "bgm3.mp3", name: "bgm3", size: 2 },
];

describe("soundOptions", () => {
  it("leads with the none entry so a sound can be cleared", () => {
    const options = soundOptions("", SOUNDS, "（なし）");
    expect(options[0]).toEqual({ value: "", label: "（なし）" });
    expect(options.map((o) => o.value)).toEqual(["", "chime.wav", "bgm3.mp3"]);
  });

  it("keeps a legacy path as its own option so the select can render it", () => {
    const options = soundOptions("sound/bgm3.mp3", SOUNDS, "（なし）");
    expect(options.map((o) => o.value)).toEqual(["", "chime.wav", "bgm3.mp3", "sound/bgm3.mp3"]);
  });

  it("does not duplicate a value that is already a library file", () => {
    const options = soundOptions("chime.wav", SOUNDS, "（なし）");
    expect(options.filter((o) => o.value === "chime.wav")).toHaveLength(1);
  });

  it("still offers the current value when the library is empty or unreadable", () => {
    // What the UI renders before /api/sound resolves, and after it fails.
    const options = soundOptions("chime.wav", [], "（なし）");
    expect(options.map((o) => o.value)).toEqual(["", "chime.wav"]);
  });
});
