// Unit tests for useNpcSocket's one piece of exported decision logic:
// nextTtsSuspended, the reducer behind the `ttsSuspended` state that the 音声
// panel's Pause/Resume buttons use for feedback (see its doc comment in this
// hook, and lib/types.ts's SuspendAcceptedMessage/ResumeAcceptedMessage for
// why this pair of acks is worth tracking at all).
import { describe, expect, it } from "vitest";
import type { ServerMessage } from "../lib/types";
import { nextTtsSuspended } from "./useNpcSocket";

describe("nextTtsSuspended", () => {
  it("turns true on suspendAccepted, regardless of the prior value", () => {
    const msg: ServerMessage = { type: "suspendAccepted" };
    expect(nextTtsSuspended(null, msg)).toBe(true);
    expect(nextTtsSuspended(false, msg)).toBe(true);
    expect(nextTtsSuspended(true, msg)).toBe(true);
  });

  it("turns false on resumeAccepted, regardless of the prior value", () => {
    const msg: ServerMessage = { type: "resumeAccepted" };
    expect(nextTtsSuspended(null, msg)).toBe(false);
    expect(nextTtsSuspended(true, msg)).toBe(false);
    expect(nextTtsSuspended(false, msg)).toBe(false);
  });

  it("leaves the current value alone for any other frame type", () => {
    const msg: ServerMessage = { type: "voice", active: true };
    expect(nextTtsSuspended(null, msg)).toBe(null);
    expect(nextTtsSuspended(true, msg)).toBe(true);
    expect(nextTtsSuspended(false, msg)).toBe(false);
  });
});
