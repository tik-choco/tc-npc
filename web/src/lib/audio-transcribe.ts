// Audio → text for the チャット composer's file-drop entry point (dropped
// recordings) — mirrors tc-assistant2's `transcribeAndFill` flow
// (src/main.tsx), but the actual model call is server-side: see
// api.ts's transcribeAudio for why (masked provider api keys) and its
// proposed `/api/llm/transcribe/:filename` contract, which does not exist
// on the server yet.
import { transcribeAudio } from "./api";

/** `MediaRecorder.mimeType` → a plausible file extension, for naming a
 *  dropped/recorded blob before it's uploaded (the server never sees a
 *  browser-native File for a mic recording, only a Blob it built itself).
 *  Falls back to `.webm`, which is what every browser's MediaRecorder
 *  actually produces today when no mimeType is requested. */
export function recordingFilename(mimeType: string): string {
  const base = mimeType.split(";")[0]?.trim().toLowerCase() ?? "";
  const extension = base === "audio/ogg" ? "ogg" : base === "audio/mp4" ? "m4a" : base === "audio/wav" ? "wav" : "webm";
  return `recording.${extension}`;
}

/** Uploads `blob` (a dropped file or a mic recording) for transcription and
 *  returns the trimmed transcript text, or `""` if the model returned
 *  nothing usable. `upload` defaults to api.ts's transcribeAudio and is
 *  injected only so this trim/unwrap logic is unit-testable without a real
 *  network call — see pdf-ocr.ts's ocrPdfImages for the same pattern. */
export async function transcribeAudioBlob(
  blob: Blob,
  filename: string,
  upload: (blob: Blob, filename: string) => Promise<{ text: string }> = transcribeAudio,
): Promise<string> {
  const { text } = await upload(blob, filename);
  return text.trim();
}
