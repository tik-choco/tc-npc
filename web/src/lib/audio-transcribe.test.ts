// Unit tests for audio-transcribe.ts. transcribeAudioBlob takes its upload
// function as an injectable parameter (defaulting to api.ts's
// transcribeAudio — see pdf-ocr.ts's ocrPdfImages for the same pattern), so
// this can exercise the trim/unwrap logic with a fake upload instead of a
// real network call. recordingFilename is pure and tested directly.
import { describe, expect, it, vi } from "vitest";
import { recordingFilename, transcribeAudioBlob } from "./audio-transcribe";

describe("recordingFilename", () => {
  it("maps audio/webm (with or without a codecs suffix) to .webm", () => {
    expect(recordingFilename("audio/webm")).toBe("recording.webm");
    expect(recordingFilename("audio/webm;codecs=opus")).toBe("recording.webm");
  });

  it("maps audio/ogg to .ogg", () => {
    expect(recordingFilename("audio/ogg;codecs=opus")).toBe("recording.ogg");
  });

  it("maps audio/mp4 to .m4a", () => {
    expect(recordingFilename("audio/mp4")).toBe("recording.m4a");
  });

  it("maps audio/wav to .wav", () => {
    expect(recordingFilename("audio/wav")).toBe("recording.wav");
  });

  it("falls back to .webm for an unrecognized or empty mimeType", () => {
    expect(recordingFilename("")).toBe("recording.webm");
    expect(recordingFilename("audio/x-mystery")).toBe("recording.webm");
  });

  it("is case-insensitive", () => {
    expect(recordingFilename("AUDIO/OGG")).toBe("recording.ogg");
  });
});

describe("transcribeAudioBlob", () => {
  it("returns the trimmed transcript text", async () => {
    const upload = vi.fn(async () => ({ text: "  hello world  \n" }));
    const blob = new Blob(["x"], { type: "audio/webm" });
    const result = await transcribeAudioBlob(blob, "recording.webm", upload);
    expect(result).toBe("hello world");
    expect(upload).toHaveBeenCalledWith(blob, "recording.webm");
  });

  it("returns an empty string when the transcript is blank", async () => {
    const upload = vi.fn(async () => ({ text: "   " }));
    const result = await transcribeAudioBlob(new Blob(["x"]), "f.webm", upload);
    expect(result).toBe("");
  });
});
