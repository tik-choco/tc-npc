// The NPC's VRM avatar on a live canvas, with the mouth tracking the host's
// actual voice loudness while speaking (see `speaking`/`speakingLevel`) and
// the face carrying the NPC's current expression.
//
// Deliberately thin: every line of three.js lives in vrm/mount.ts, which
// this reaches through a dynamic `import()` so the (large) 3D stack is a
// separate chunk that a session which never opens an avatar layout never
// downloads. What is left here is the Preact shell — the element to draw
// into, the load/error state, and the pointer gestures that drive the camera
// (plus persisting where those gestures left it — see the camera-memory
// section below).
//
// Everything the render loop reads lives in a ref rather than being closed
// over: `speaking`/`speakingLevel` flip on every TTS clip and `emotion` on
// every turn, and re-running the (expensive) model load on each of those
// would restart the avatar mid-sentence.
import { useEffect, useRef, useState } from "preact/hooks";
import type { OrbitState, VrmFraming } from "../vrm/stage";
import type { EmotionName } from "../vrm/animation";
import type { MountedAvatar } from "../vrm/mount";
// A value import, unlike the others here — but vrm/level.ts is deliberately
// dependency-free, so it doesn't pull the 3D stack out of its lazy chunk.
import { IDLE_SPEAKING_LEVEL, type SpeakingLevelReading } from "../vrm/level";
import "../styles/avatar.css";

export interface VrmStageProps {
  /** Model file name in the server's VRM folder (`GET /api/vrm/file/:file`). */
  file: string;
  /** True while the NPC's voice is audible on the host — drives the mouth. */
  speaking?: boolean;
  /**
   * Live loudness of the voice currently audible, as a ref rather than a
   * value. Readings land ~20 times a second, and routing them through props
   * would re-render this component — and everything above it — at that rate
   * purely to feed a render loop that reads the value itself. The ref lets
   * the loop see every reading while Preact sees none of them.
   */
  speakingLevelRef?: { current: SpeakingLevelReading };
  /** Current facial expression, or null/"neutral" for none. */
  emotion?: EmotionName | null;
  /** Body crop. Defaults to the head-to-hips "upper" shot. */
  framing?: VrmFraming;
  /** Allow drag-to-orbit / wheel-to-zoom / right-drag-to-pan. */
  interactive?: boolean;
  /** Fallback glyph shown while the model loads, and if it fails. */
  initial?: string;
  class?: string;
}

// --- Camera memory --------------------------------------------------------
// The orbit (drag/zoom/pan) is otherwise reset to the default framing on
// every reload, which is annoying for a window someone leaves open on a
// second monitor. Persisted per model file *and* framing — bust/upper/full
// crop the shot completely differently, so one saved angle can't sensibly
// serve all three.
const CAMERA_STORAGE_PREFIX = "tc-npc:vrm-camera:";

// Wheel events fire once per scroll notch, far too often to hit localStorage
// on each one — writes are coalesced to the first quiet moment after
// scrolling stops (drag/pan/reset already save on their own single discrete
// end-of-gesture event, so they don't need this).
const WHEEL_SAVE_DEBOUNCE_MS = 400;

function cameraStorageKey(file: string, framing: VrmFraming): string {
  return `${CAMERA_STORAGE_PREFIX}${file}:${framing}`;
}

/** Defensive the same way lib/router.ts's chat-panel/layout loaders are:
 *  localStorage can throw (private browsing), and a stored value might be
 *  from an older build or hand-edited into nonsense. Either way this just
 *  returns undefined and mountAvatar() keeps the default framing — the
 *  actual protection against an out-of-range value living inside that
 *  default is `orbit.restore()`'s own clamping (see vrm/stage.ts), since a
 *  shape check here can't catch a merely-out-of-bounds number. */
function loadStoredOrbit(file: string, framing: VrmFraming): OrbitState | undefined {
  try {
    const raw = localStorage.getItem(cameraStorageKey(file, framing));
    if (!raw) return undefined;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return undefined;
    return parsed as OrbitState;
  } catch {
    return undefined;
  }
}

function saveOrbit(file: string, framing: VrmFraming, state: OrbitState): void {
  try {
    localStorage.setItem(cameraStorageKey(file, framing), JSON.stringify(state));
  } catch {
    // Non-fatal — the camera just won't be remembered next time.
  }
}

export function VrmStage({
  file,
  speaking,
  speakingLevelRef,
  emotion,
  framing = "upper",
  interactive,
  initial,
  class: className,
}: VrmStageProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const mountedRef = useRef<MountedAvatar | null>(null);
  const [status, setStatus] = useState<"loading" | "ready" | "error">("loading");

  const speakingRef = useRef(false);
  speakingRef.current = speaking ?? false;
  const emotionRef = useRef<EmotionName | null>(null);
  emotionRef.current = emotion ?? null;

  // Drag bookkeeping lives in a ref, not state — pointermove fires far too
  // often to route through Preact's render cycle.
  const dragRef = useRef<{ pointerId: number; lastX: number; lastY: number; mode: "rotate" | "pan" } | null>(null);
  const wheelSaveTimerRef = useRef<number | undefined>(undefined);

  // Save whatever the orbit currently is under this file+framing's key. Cheap
  // enough to call on every discrete gesture-end event (pointerup, a settled
  // wheel burst, dblclick-reset) — it's the continuous pointermove path that
  // must never reach localStorage, not this.
  const persistOrbit = () => {
    const mounted = mountedRef.current;
    if (!mounted) return;
    saveOrbit(file, framing, mounted.orbit.snapshot());
  };

  useEffect(() => () => window.clearTimeout(wheelSaveTimerRef.current), []);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    let cancelled = false;
    setStatus("loading");

    void (async () => {
      try {
        const { mountAvatar } = await import("../vrm/mount");
        // The chunk fetch and the model load are both async, so the effect
        // may well have been torn down by the time either resolves.
        if (cancelled) return;
        const mounted = await mountAvatar(container, {
          file,
          framing,
          getSpeaking: () => speakingRef.current,
          // Falls back to the idle reading when no ref was supplied: the
          // animator then never sees a new `seq` and drives the mouth from
          // its own cadence, which is exactly what a caller that can't
          // provide loudness wants.
          getSpeakingLevel: () => speakingLevelRef?.current ?? IDLE_SPEAKING_LEVEL,
          getEmotion: () => emotionRef.current,
          initialOrbit: loadStoredOrbit(file, framing),
        });
        if (cancelled) {
          mounted.dispose();
          return;
        }
        mountedRef.current = mounted;
        setStatus("ready");
      } catch (err) {
        if (cancelled) return;
        console.error("failed to mount VRM avatar", err);
        setStatus("error");
      }
    })();

    return () => {
      cancelled = true;
      mountedRef.current?.dispose();
      mountedRef.current = null;
    };
  }, [file, framing]);

  const canOrbit = Boolean(interactive) && status === "ready";

  const endDrag = (e: PointerEvent) => {
    const el = e.currentTarget as HTMLElement;
    if (dragRef.current?.pointerId === e.pointerId) {
      dragRef.current = null;
      el.style.cursor = "grab";
      // Gesture end: exactly the point to persist, whether this drag
      // actually moved the camera or was just a click that never left the
      // pointerdown/pointerup pair — writing the unchanged snapshot again is
      // harmless and far cheaper than tracking "did it really move".
      persistOrbit();
    }
    if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId);
  };

  const handlePointerDown = (e: PointerEvent) => {
    if (!canOrbit) return;
    const el = e.currentTarget as HTMLElement;
    // Right button (or Shift+drag, for trackpads without one) pans; plain
    // left drag orbits.
    const mode = e.button === 2 || e.shiftKey ? "pan" : "rotate";
    dragRef.current = { pointerId: e.pointerId, lastX: e.clientX, lastY: e.clientY, mode };
    el.setPointerCapture(e.pointerId);
    el.style.cursor = mode === "pan" ? "move" : "grabbing";
  };

  const handlePointerMove = (e: PointerEvent) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== e.pointerId) return;
    const dx = e.clientX - drag.lastX;
    const dy = e.clientY - drag.lastY;
    drag.lastX = e.clientX;
    drag.lastY = e.clientY;
    if (drag.mode === "pan") {
      // Normalized by the viewport height so panBy moves the pivot 1:1 with
      // the cursor at any stage size.
      const height = (e.currentTarget as HTMLElement).clientHeight || 1;
      mountedRef.current?.orbit.panBy(dx / height, dy / height);
    } else {
      mountedRef.current?.orbit.rotateBy(-dx * 0.008, dy * 0.006);
    }
  };

  const handleWheel = (e: WheelEvent) => {
    if (!canOrbit) return;
    // preventDefault keeps the page from scrolling while the cursor is over
    // the avatar; the exp() mapping makes each notch a constant zoom ratio
    // regardless of direction or device delta scale.
    e.preventDefault();
    mountedRef.current?.orbit.zoomBy(Math.exp(e.deltaY * 0.0012));
    // A scroll gesture is a burst of many wheel events, not one — debounce so
    // only the first quiet moment after the burst hits localStorage instead
    // of every single notch.
    window.clearTimeout(wheelSaveTimerRef.current);
    wheelSaveTimerRef.current = window.setTimeout(persistOrbit, WHEEL_SAVE_DEBOUNCE_MS);
  };

  return (
    <div
      ref={containerRef}
      class={`vrm-stage${status !== "ready" ? " vrm-stage--pending" : ""}${className ? ` ${className}` : ""}`}
      style={canOrbit ? { touchAction: "none", cursor: "grab" } : undefined}
      onPointerDown={canOrbit ? handlePointerDown : undefined}
      onPointerMove={canOrbit ? handlePointerMove : undefined}
      onPointerUp={canOrbit ? endDrag : undefined}
      onPointerCancel={canOrbit ? endDrag : undefined}
      onDblClick={
        canOrbit
          ? () => {
              mountedRef.current?.orbit.reset();
              persistOrbit();
            }
          : undefined
      }
      onWheel={canOrbit ? handleWheel : undefined}
      // Right-drag pans, so the browser context menu must not pop on release.
      onContextMenu={canOrbit ? (e: MouseEvent) => e.preventDefault() : undefined}
    >
      {status !== "ready" && <span class="vrm-stage-fallback">{initial ?? "N"}</span>}
    </div>
  );
}
