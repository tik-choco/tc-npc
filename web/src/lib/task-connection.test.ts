import { describe, expect, it } from "vitest";
import { isTaskConfigured } from "./task-connection";
import type { ConfigDocument } from "./types";
const config = (): ConfigDocument => ({ providers: [
  { id: "http", base_url: "http://example.test/v1", api_key: "" },
  { id: "disabled", base_url: "http://disabled.test/v1", enabled: false },
  { id: "room", base_url: "mist-network://team" },
], default_ref: { provider_id: "http", model: "default" } });
describe("ModelRef task readiness", () => {
  it("requires a usable ref or default, not a direct legacy endpoint", () => {
    expect(isTaskConfigured(null, "vision")).toBe(false);
    expect(isTaskConfigured({ vision: { base_url: "http://legacy.test" } }, "vision")).toBe(false);
    expect(isTaskConfigured(config(), "stt")).toBe(true);
  });
  it("accepts room refs and HTTP refs without keys", () => {
    const doc = config(); delete doc.default_ref; doc.vision = { model_ref: { provider_id: "room", model: "remote" } };
    expect(isTaskConfigured(doc, "vision")).toBe(true);
  });
  it("falls back only to a usable default without rewriting disabled refs", () => {
    const doc = config(); doc.stt = { model_ref: { provider_id: "disabled", model: "private" } };
    const before = structuredClone(doc.stt); expect(isTaskConfigured(doc, "stt")).toBe(true); expect(doc.stt).toEqual(before);
    delete doc.default_ref; expect(isTaskConfigured(doc, "stt")).toBe(false);
  });
});