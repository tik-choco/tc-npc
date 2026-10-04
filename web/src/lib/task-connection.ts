import { resolveTaskModel, type LlmTaskId } from "./llm-config";
import type { ConfigDocument } from "./types";
// All inference remains on REST; this gate mirrors Rust ModelRef resolution.
export function isTaskConfigured(config: ConfigDocument | null, task: LlmTaskId): boolean {
  return resolveTaskModel(config, task) !== null;
}