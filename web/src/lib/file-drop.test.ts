// Unit tests for file-drop.ts's pure file-type sniffing. The risky parts are
// the two fallback paths that don't rely on `file.type`: the audio extension
// list (some platforms leave `type` empty for less common containers) and
// the PDF filename suffix check.
import { describe, expect, it } from "vitest";
import { isAudioFile, isPdfFile } from "./file-drop";

function file(name: string, type: string): File {
  return new File(["x"], name, { type });
}

describe("isAudioFile", () => {
  it("accepts any file whose type starts with audio/", () => {
    expect(isAudioFile(file("clip.bin", "audio/webm"))).toBe(true);
    expect(isAudioFile(file("clip.bin", "audio/x-custom"))).toBe(true);
  });

  it("falls back to the extension list when type is empty", () => {
    expect(isAudioFile(file("voice.mp3", ""))).toBe(true);
    expect(isAudioFile(file("voice.WAV", ""))).toBe(true);
    expect(isAudioFile(file("voice.m4a", ""))).toBe(true);
  });

  it("rejects a non-audio type with a non-audio extension", () => {
    expect(isAudioFile(file("notes.pdf", "application/pdf"))).toBe(false);
    expect(isAudioFile(file("photo.png", "image/png"))).toBe(false);
  });

  it("rejects a file with no extension and no recognized type", () => {
    expect(isAudioFile(file("README", ""))).toBe(false);
  });
});

describe("isPdfFile", () => {
  it("accepts application/pdf regardless of file name", () => {
    expect(isPdfFile(file("scan", "application/pdf"))).toBe(true);
  });

  it("falls back to a case-insensitive .pdf suffix when type is empty", () => {
    expect(isPdfFile(file("scan.PDF", ""))).toBe(true);
  });

  it("rejects a non-pdf file", () => {
    expect(isPdfFile(file("clip.mp3", "audio/mpeg"))).toBe(false);
  });
});
