// Detects whether a `File` dropped onto the チャット composer is something the
// composer knows how to turn into text: an audio recording (→ STT, see
// audio-transcribe.ts) or a PDF (→ per-page OCR, see pdf-ocr.ts). Ported from
// tc-assistant2/src/files.ts — same extension list and MIME sniffing, just
// relocated under tc-npc's web/src/lib naming (no ApiProfile/Tauri coupling
// to bring along, this module is pure file-type detection).

/** Extensions accepted as "audio" when the browser doesn't set a `type` at
 *  all (a local file picker on some platforms leaves `file.type` empty for
 *  less common containers) — matches tc-assistant2's constants.ts list. */
const AUDIO_EXTENSIONS = new Set([
  "aac",
  "aiff",
  "flac",
  "m4a",
  "mp3",
  "mp4",
  "mpeg",
  "oga",
  "ogg",
  "opus",
  "wav",
  "webm",
]);

export function isAudioFile(file: File): boolean {
  if (file.type.startsWith("audio/")) {
    return true;
  }

  const extension = file.name.split(".").pop()?.toLowerCase();
  return extension ? AUDIO_EXTENSIONS.has(extension) : false;
}

export function isPdfFile(file: File): boolean {
  return file.type === "application/pdf" || file.name.toLowerCase().endsWith(".pdf");
}
