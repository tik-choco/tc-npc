// Unit tests for VoicePanel's one piece of exported decision logic:
// ttsLogEmptyKey, which picks the empty-state copy for the TTS log now that
// `ttsLine` is sent per sentence (see lib/types.ts's TtsLineMessage) and is
// therefore correctly empty whenever TTS itself is off, not just whenever
// nobody has spoken yet.
import { describe, expect, it } from "vitest";
import type { ConfigDocument } from "../lib/types";
import { ttsLogEmptyKey } from "./VoicePanel";

describe("ttsLogEmptyKey", () => {
  it("says TTS is off when config.tts.enabled is explicitly false", () => {
    const config: ConfigDocument = { tts: { enabled: false } };
    expect(ttsLogEmptyKey(config)).toBe("voice.log.empty.ttsOff");
  });

  it("falls back to the generic empty line when config.tts.enabled is true", () => {
    const config: ConfigDocument = { tts: { enabled: true } };
    expect(ttsLogEmptyKey(config)).toBe("voice.log.empty");
  });

  it("treats a missing tts section as off, matching SettingsView's own tts.enabled ?? false", () => {
    const config: ConfigDocument = {};
    expect(ttsLogEmptyKey(config)).toBe("voice.log.empty.ttsOff");
  });

  it("does not claim TTS is off while the config document hasn't loaded yet", () => {
    expect(ttsLogEmptyKey(null)).toBe("voice.log.empty");
  });
});
