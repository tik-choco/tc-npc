// The NPC's current speech loudness, as read by the avatar's render loop.
//
// This lives in its own module rather than in animation.ts because both the
// socket hook (which produces the reading) and the animator (which consumes
// it) need the type, and animation.ts imports three.js — a type-only import
// erases at build time, but one careless edit turning it into a value import
// would drag the whole 3D stack into the main bundle, which the dynamic
// `import()` in components/VrmStage.tsx exists specifically to prevent. A
// dependency-free module can't do that.

/**
 * One loudness reading from the server's `speakingLevel` frame.
 *
 * `seq` increments on every reading — including one whose `level` happens to
 * repeat the previous value. That is the whole reason it exists: the render
 * loop runs at ~60Hz while readings arrive at ~20Hz, so the animator has to
 * distinguish "the same value arrived again" from "no new value has arrived",
 * and it cannot do that by comparing `level` alone. Silence sends a genuine
 * run of identical `0`s, and a loud passage sends a genuine run of identical
 * `1`s (the server clamps), so value-comparison would read both of those —
 * the two most common cases — as a stalled feed and drop the mouth into its
 * fallback animation, flapping through the exact silences it is supposed to
 * stay shut for.
 */
export interface SpeakingLevelReading {
  /** Loudness, 0..=1. Meaningless while the NPC isn't speaking. */
  level: number;
  /** Monotonic counter, bumped once per reading. Never reset. */
  seq: number;
}

/** The reading to start from, before any frame has arrived. */
export const IDLE_SPEAKING_LEVEL: SpeakingLevelReading = { level: 0, seq: 0 };
