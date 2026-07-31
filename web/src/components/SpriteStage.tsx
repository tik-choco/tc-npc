// The NPC's 2D body: one PNG sprite sheet, animated by shifting a
// background offset. The second display mode alongside `VrmStage`.
//
// Why it exists: a VRM is a 3D scene — a large lazy chunk, a WebGL context,
// and a machine able to drive both. A sheet is an image. That makes this the
// display mode that works when the other one can't, and the one a character
// with no model of its own can still use.
//
// The two stages are deliberately interchangeable from the caller's side:
// `AvatarView` picks between them on the avatar reference's `kind` and hands
// over the same `speaking`/`speakingLevelRef`/`emotion` inputs. Anything
// added to that contract should land on both, or on neither.
//
// ## Sheet convention
//
// A sheet is a grid of equal cells, `SHEET_COLUMNS` wide. Rows are frame
// strips, played left to right and looped:
//
//   row 0 — idle. Always present; a one-cell sheet is a still image.
//   row 1 — speaking, used while the host's voice is audible. Optional: a
//           sheet with only one row simply keeps playing idle, which is why
//           the row is chosen from the sheet's measured height rather than
//           assumed.
//
// Nothing else is assigned meaning. The sheets tc-town exports are a grid of
// 128 px cells and carry no per-row semantics of their own, so reading more
// structure into them than this would be inventing a contract the files
// don't actually keep.
//
// Like `VrmStage`, the animation loop reads its inputs from refs: the
// loudness reading lands ~20 times a second and `speaking` flips on every
// clip, and re-rendering at that rate to feed a loop that can read the
// values itself would be pure waste.
import { useEffect, useRef, useState } from "preact/hooks";
import { IDLE_SPEAKING_LEVEL, type SpeakingLevelReading } from "../vrm/level";
import "../styles/avatar.css";

/** Cells per row, matching the sheets tc-town's exporter produces. */
const SHEET_COLUMNS = 8;

/** Frames per second for the idle strip. Slow: it is a breathing loop. */
const IDLE_FPS = 6;

/**
 * Frames per second for the speaking strip at full loudness. The rate is
 * scaled by the live level so the mouth tracks the actual speech envelope,
 * the same input `VrmStage` opens its mouth with — a fixed rate reads as a
 * flapping puppet that ignores what is being said.
 */
const SPEAKING_FPS_MAX = 14;
/** Floor so a quiet passage still animates rather than freezing mid-word. */
const SPEAKING_FPS_MIN = 5;

export interface SpriteStageProps {
  /** Sheet file name in the server's sprite folder (`GET /api/sprites/file/:file`). */
  file: string;
  /** True while the NPC's voice is audible on the host — picks the strip. */
  speaking?: boolean;
  /**
   * Live loudness of the voice currently audible, as a ref rather than a
   * value — see the note at the top of this file, and `VrmStage`, which
   * takes the identical input for the identical reason.
   */
  speakingLevelRef?: { current: SpeakingLevelReading };
  /** Accepted for parity with `VrmStage`; a flat sheet has no expressions. */
  emotion?: string | null;
  /** Accessible label — the character's name when there is one. */
  label?: string;
}

export function SpriteStage({ file, speaking, speakingLevelRef, label }: SpriteStageProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const [status, setStatus] = useState<"loading" | "ready" | "error">("loading");
  /** Measured from the loaded image, so a sheet may have any number of rows. */
  const rowsRef = useRef(1);
  const cellRef = useRef({ width: 0, height: 0 });

  // Inputs the loop reads without re-rendering.
  const speakingRef = useRef(false);
  speakingRef.current = speaking ?? false;

  const src = `/api/sprites/file/${encodeURIComponent(file)}`;

  // Load the sheet and measure its grid. Re-runs only when the file changes:
  // a new loudness reading must never restart the image load.
  useEffect(() => {
    let cancelled = false;
    setStatus("loading");

    const image = new Image();
    image.onload = () => {
      if (cancelled) return;
      const cellWidth = image.naturalWidth / SHEET_COLUMNS;
      // Cells are square in every sheet we produce, so the row count follows
      // from the height rather than being configured. A sheet whose height
      // isn't a whole number of cells still works — it just has a partial
      // last row that never gets selected.
      const rows = Math.max(1, Math.floor(image.naturalHeight / cellWidth));
      cellRef.current = { width: cellWidth, height: cellWidth };
      rowsRef.current = rows;
      setStatus("ready");
    };
    image.onerror = () => {
      if (!cancelled) setStatus("error");
    };
    image.src = src;

    return () => {
      cancelled = true;
    };
  }, [src]);

  // The animation loop. Started only once the sheet has been measured,
  // because the offsets it writes are meaningless before then.
  useEffect(() => {
    if (status !== "ready") return;
    const host = hostRef.current;
    if (!host) return;

    let frame = 0;
    let column = 0;
    let lastStep = 0;

    const step = (now: number) => {
      const isSpeaking = speakingRef.current;
      // Row 1 only exists on sheets that have it; otherwise idle plays
      // throughout, which is the correct degradation for a still image.
      const row = isSpeaking && rowsRef.current > 1 ? 1 : 0;

      const level = speakingLevelRef?.current?.level ?? IDLE_SPEAKING_LEVEL.level;
      const fps = isSpeaking
        ? SPEAKING_FPS_MIN + (SPEAKING_FPS_MAX - SPEAKING_FPS_MIN) * clamp01(level)
        : IDLE_FPS;

      if (now - lastStep >= 1000 / fps) {
        column = (column + 1) % SHEET_COLUMNS;
        lastStep = now;
        const { width, height } = cellRef.current;
        host.style.backgroundPosition = `-${column * width}px -${row * height}px`;
      }
      frame = requestAnimationFrame(step);
    };

    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  }, [status, speakingLevelRef]);

  if (status === "error") {
    // Same posture as a dangling VRM reference: a missing sheet is not an
    // error state to shout about, it just means there is nothing to draw.
    return <div class="sprite-stage sprite-stage--missing" aria-hidden="true" />;
  }

  const { width, height } = cellRef.current;
  return (
    <div
      ref={hostRef}
      class={status === "ready" ? "sprite-stage" : "sprite-stage sprite-stage--loading"}
      role="img"
      aria-label={label ?? "Avatar"}
      style={{
        backgroundImage: `url(${src})`,
        // The element shows exactly one cell; the sheet is positioned behind
        // it and slid, rather than being scaled to fit.
        width: width ? `${width}px` : undefined,
        height: height ? `${height}px` : undefined,
      }}
    />
  );
}

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return value < 0 ? 0 : value > 1 ? 1 : value;
}
