// Unit tests for AvatarView's exported decision logic. Same shape as
// VoicePanel.test.ts: the component itself needs a DOM, but the choices it
// makes are pure and are where the mistakes would be.
import { describe, expect, it } from "vitest";
import { AvatarDragHandle, captionStyleFromStored, deriveAvatarStatus } from "./AvatarView";

describe("captionStyleFromStored", () => {
  // The important case. This key held "1"/"0" while captions were on/off,
  // and this window is set up once and left running as a capture source —
  // dropping an old value would silently undo that setup on the next reload.
  it("keeps a preference written before the bubble existed", () => {
    expect(captionStyleFromStored("1")).toBe("strip");
    expect(captionStyleFromStored("0")).toBe("off");
  });

  it("round-trips every current value", () => {
    expect(captionStyleFromStored("off")).toBe("off");
    expect(captionStyleFromStored("strip")).toBe("strip");
    expect(captionStyleFromStored("bubble")).toBe("bubble");
  });

  // A hand-edited value, or one written by a newer build, must not leave the
  // window with a style it can't render.
  it("falls back to the default for anything unrecognised", () => {
    expect(captionStyleFromStored(null)).toBe("strip");
    expect(captionStyleFromStored("")).toBe("strip");
    expect(captionStyleFromStored("balloon")).toBe("strip");
    expect(captionStyleFromStored("2")).toBe("strip");
  });
});

describe("deriveAvatarStatus", () => {
  // Guards the precedence documented on the function: speaking wins over a
  // mic that is still open underneath it (barge-in keeps it open), because
  // the indicator exists to explain the mouth motion already on screen.
  it("reports speaking even while the mic is still listening", () => {
    expect(deriveAvatarStatus({ voiceActive: true, pending: false, speaking: true })).toBe(
      "speaking",
    );
  });

  it("reports thinking for a pending reply with the voice loop off", () => {
    expect(deriveAvatarStatus({ voiceActive: false, pending: true, speaking: false })).toBe(
      "thinking",
    );
  });

  it("reports listening only once nothing else is happening", () => {
    expect(deriveAvatarStatus({ voiceActive: true, pending: false, speaking: false })).toBe(
      "listening",
    );
  });

  it("is idle with the loop off and nothing in flight", () => {
    expect(deriveAvatarStatus({ voiceActive: false, pending: false, speaking: false })).toBe(
      "idle",
    );
  });
});

describe("AvatarDragHandle", () => {
  // AvatarDragHandle is deliberately hook-free (unlike AvatarView itself),
  // so — like AvatarStatusBadge — it can be invoked directly as a plain
  // function here: JSX just builds a plain vnode object (`{ type, props }`),
  // and building that object needs no DOM or render pass, only a rendering
  // context if hooks are involved. That lets this window's move handle be
  // asserted on without pulling in a DOM test environment this project
  // doesn't otherwise need.

  it("carries Tauri's drag-region marker when it renders", () => {
    const vnode = AvatarDragHandle({ bare: true });
    expect(vnode).not.toBeNull();
    expect(vnode?.type).toBe("div");
    expect(vnode?.props.class).toBe("avatar-window-drag");
    // Asserted exactly, not for truthiness: the value is what was wrong the
    // first time. Tauri's drag.js reads a *bare* attribute as "only a direct
    // click on this element counts" (`return el === composedPath[0]`), which
    // left the grip span inside the strip as the one place a drag wouldn't
    // start — the affordance that says "grab here" being the only spot that
    // didn't. Caught by synthetic drag: on the grip the window didn't move,
    // 60px to its left it tracked the cursor exactly. `deep` opts the whole
    // subtree in, so a decorative child can't break the handle again.
    expect(vnode?.props["data-tauri-drag-region"]).toBe("deep");
  });

  // The whole point of confining this to a `bare` flag: a caller that is
  // not the popped-out `#/avatar` window must render nothing here, so a
  // click inside it can never be hijacked into an OS-level window drag.
  it("renders nothing when the caller is not the bare avatar window", () => {
    expect(AvatarDragHandle({ bare: false })).toBeNull();
  });
});
