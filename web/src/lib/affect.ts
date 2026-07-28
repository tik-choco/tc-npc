// Shared affect-model presentation helpers.
//
// The 22-drive model is now rendered in two places at two levels of detail:
// the 感情 tab (BrainView) shows every drive with baseline markers and a
// trend sparkline, while the チャット tab's 状態 panel (StatusPanel) shows a
// condensed "内心" block — familiarity plus the handful of drives currently
// deviating enough to reach the persona prompt. Both need the same label
// map, the same thresholds, and the same notion of "which drives are
// actually active right now", so those live here rather than being copied
// (and drifting) between the two views.
import type { AffectSnapshot } from "../hooks/useNpcSocket";
import type { MessageKey, Translate } from "./i18n";

/** Fixed display order for the 22 drives — matches the WS frame's own fixed
 *  order (see lib/types.ts's AffectMessage doc comment). Kept as an explicit
 *  list (rather than derived from the first frame) so the 感情 tab's trend
 *  dropdown and legend have something to render before any frame arrives. */
export const DRIVE_ORDER = [
  "dopamine",
  "serotonin",
  "oxytocin",
  "endorphin",
  "cortisol",
  "noradrenaline",
  "adrenaline",
  "acetylcholine",
  "glutamate",
  "gaba",
  "glycine",
  "melatonin",
  "orexin",
  "histamine",
  "dynorphin",
  "dhea",
  "enkephalin",
  "anandamide",
  "substance_p",
  "npy",
  "cck",
  "bdnf",
] as const;

export const DRIVE_LABEL_KEYS: Record<string, MessageKey> = {
  dopamine: "brain.drive.dopamine",
  serotonin: "brain.drive.serotonin",
  oxytocin: "brain.drive.oxytocin",
  endorphin: "brain.drive.endorphin",
  cortisol: "brain.drive.cortisol",
  noradrenaline: "brain.drive.noradrenaline",
  adrenaline: "brain.drive.adrenaline",
  acetylcholine: "brain.drive.acetylcholine",
  glutamate: "brain.drive.glutamate",
  gaba: "brain.drive.gaba",
  glycine: "brain.drive.glycine",
  melatonin: "brain.drive.melatonin",
  orexin: "brain.drive.orexin",
  histamine: "brain.drive.histamine",
  dynorphin: "brain.drive.dynorphin",
  dhea: "brain.drive.dhea",
  enkephalin: "brain.drive.enkephalin",
  anandamide: "brain.drive.anandamide",
  substance_p: "brain.drive.substance_p",
  npy: "brain.drive.npy",
  cck: "brain.drive.cck",
  bdnf: "brain.drive.bdnf",
};

// The affect engine folds the 3 drives with |level - base| >= this threshold
// into the persona prompt each turn — kept in sync with the Rust side by
// convention rather than sent over the wire.
export const DEVIATION_THRESHOLD = 0.2;
export const FAMILIARITY_WARY_THRESHOLD = 0.4;

/** How many drives the engine actually folds into the prompt — also the cap
 *  the condensed 内心 block renders, so it shows exactly the set that is
 *  shaping the reply and nothing more. */
export const PROMPT_DRIVE_COUNT = 3;

export function driveLabel(t: Translate, key: string): string {
  const msgKey = DRIVE_LABEL_KEYS[key];
  return msgKey ? t(msgKey) : key;
}

export function clampPct(v: number): number {
  return Math.round(Math.max(0, Math.min(1, v)) * 100);
}

export interface DriveDeviation {
  key: string;
  level: number;
  base: number;
  /** Signed `level - base`; the sign is what the up/down styling keys off. */
  delta: number;
  /** `|delta|`, i.e. the value the ranking sorts by. */
  deviation: number;
}

/** Drives currently deviating at least `DEVIATION_THRESHOLD` from baseline,
 *  strongest first. An empty result means the NPC is sitting at baseline —
 *  a real state worth showing, not an error. */
export function rankDeviations(affect: AffectSnapshot | null): DriveDeviation[] {
  if (!affect) return [];
  return affect.drives
    .map((d) => ({
      key: d.key,
      level: d.level,
      base: d.base,
      delta: d.level - d.base,
      deviation: Math.abs(d.level - d.base),
    }))
    .filter((d) => d.deviation >= DEVIATION_THRESHOLD)
    .sort((a, b) => b.deviation - a.deviation);
}

/** Direction bucket for a signed delta. The dead zone keeps a drive that is
 *  a rounding error away from baseline from flickering between up and down. */
export function deltaDirection(delta: number): "up" | "down" | "flat" {
  if (delta > 0.0005) return "up";
  if (delta < -0.0005) return "down";
  return "flat";
}
