// Unit tests for formatHash, the exported half of the hash-router's
// parse/format pair (see router.ts's module doc for the route shape).
// parseHash itself, plus the localStorage-backed loadChatPanel/loadChatLayout
// fallbacks, are not exported — see the follow-up note in this worker's
// report for why those are left uncovered rather than exported just for
// testing purposes.
import { describe, expect, it } from "vitest";
import { formatHash, type Route } from "./router";

function route(overrides: Partial<Route> = {}): Route {
  return { tab: "chat", chatPanel: "voice", chatLayout: "chat", ...overrides };
}

describe("formatHash", () => {
  it("ignores chatPanel/chatLayout entirely for a non-chat tab", () => {
    expect(formatHash(route({ tab: "settings", chatPanel: "status", chatLayout: "avatar" }))).toBe("#/settings");
  });

  it("renders the bare #/chat/<panel> form when the layout is the default", () => {
    expect(formatHash(route({ chatPanel: "voice", chatLayout: "chat" }))).toBe("#/chat/voice");
  });

  it("carries a non-default sidebar panel into the hash", () => {
    expect(formatHash(route({ chatPanel: "status", chatLayout: "chat" }))).toBe("#/chat/status");
  });

  it("appends the layout segment only when it isn't the default", () => {
    expect(formatHash(route({ chatPanel: "voice", chatLayout: "avatar" }))).toBe("#/chat/voice/avatar");
  });

  it("combines a non-default panel and a non-default layout", () => {
    expect(formatHash(route({ chatPanel: "interpret", chatLayout: "avatar" }))).toBe("#/chat/interpret/avatar");
  });
});
