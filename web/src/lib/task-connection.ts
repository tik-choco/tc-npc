// "Is this task ready to actually run?" gate for the チャット composer's file
// drop → OCR/STT features (see pdf-ocr.ts, audio-transcribe.ts, ChatView.tsx).
//
// A task counts as configured the same way lib/llm-config.ts's own tasks do:
// either it has a preset assigned (its own `preset_id`, or "" following
// default_preset_id — see resolveTaskPreset), or — for vision/stt
// specifically — it falls back to a base_url set directly on its own config
// section (VisionSection/SpeechEndpointSection in config-types.ts document
// this same fallback-to-fields shape). Checked client-side purely to decide
// whether to bother the server at all: dropping a file with nothing
// configured would otherwise round-trip to a 4xx/empty result just to learn
// what a local config read already knows.
import { resolveTaskPreset, type LlmTaskId } from "./llm-config";
import type { ConfigDocument } from "./types";

export function isTaskConfigured(config: ConfigDocument | null, task: LlmTaskId): boolean {
  if (resolveTaskPreset(config, task)) return true;
  const section = (config?.[task] as Record<string, unknown> | undefined) ?? {};
  const baseUrl = section.base_url;
  return typeof baseUrl === "string" && baseUrl.trim().length > 0;
}
