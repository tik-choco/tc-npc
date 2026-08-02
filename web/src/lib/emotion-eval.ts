// Scoring harness for decideEmotion (see eval/emotion/CONTRACT.md).
//
// Two artifacts feed this file, produced by two other pieces of work this
// one is deliberately kept file-disjoint from:
//
//   eval/emotion/dataset.jsonl  — hand-labelled conversations (JSON Lines,
//     one EvalConversation per line), the gold `expect`/`alsoOk` labels.
//   eval/emotion/trace.json     — the real npc-talk affect model replayed
//     over those same conversations (`cargo run -p npc-talk --example
//     affect_trace`), one AffectSnapshot per turn.
//
// scoreTrace() joins them by conversation `id` and turn `index`, replays
// each conversation's affect frames turn-by-turn through the real
// decideEmotion — starting `previous` at `null` for every conversation
// (conversations don't share hysteresis state; each is an independent
// encounter) — and reports how often the prediction matches the label —
// strictly (must equal `expect`) and leniently (equal `expect` or listed in
// `alsoOk`). It never assumes the two inputs line up: any id or turn-count
// mismatch is recorded in `problems` rather than silently skewing the
// accuracy numbers, since a misaligned join that still produces
// plausible-looking percentages is the failure mode most likely to go
// unnoticed.
//
// Accuracy alone can't tell "the hysteresis fixed the early-conversation
// flicker" from "it didn't" — two runs can tie on accuracy while one visibly
// chatters between expressions and the other doesn't. `switches`/`flickers`
// (see countSwitchesAndFlickers below) exist to measure that directly, for
// both the model's predicted sequence and the gold label sequence, so the
// two are comparable rather than the predicted count being a number with
// nothing to judge it against.
//
// Every conversation also carries a `split: "dev" | "holdout"` (see
// CONTRACT.md's "dev / holdout split" section for why: roughly a third of
// conversations, stratified across scenario categories, held out so that
// vocabulary/threshold tuning done by eyeballing dev data has something
// independent to be checked against). scoreDataset() is the entry point
// that actually exercises this: it scores overall, dev-only, and
// holdout-only, and reports the dev-minus-holdout strict-accuracy gap
// directly — that gap is the overfitting signal, not something a caller
// should have to compute by subtracting two numbers themselves. A
// conversation with a missing or invalid `split` is excluded from every
// report and recorded in `problems`, never silently treated as "dev".
//
// This file is pure — no file I/O. The test file reads dataset.jsonl and
// trace.json off disk and hands the parsed JSON in.

import type { AffectSnapshot } from "../hooks/useNpcSocket";
import type { EmotionName } from "../vrm/animation";
import { decideEmotion, type EmotionDecision } from "./vrm-emotion";

/** The six VRM expressions the mapping can produce, gold-labelled turns are
 *  drawn from the same set. */
export const EMOTIONS: readonly EmotionName[] = ["neutral", "happy", "angry", "sad", "relaxed", "surprised"];

/** One hand-labelled turn from `dataset.jsonl`. */
export interface EvalTurn {
  speaker?: string;
  text: string;
  reply?: string;
  expect: EmotionName;
  alsoOk?: EmotionName[];
  why: string;
}

/** The two dataset partitions (see CONTRACT.md's "dev / holdout split"). */
export type EvalSplit = "dev" | "holdout";

/** One hand-labelled conversation: a line of `dataset.jsonl`. */
export interface EvalConversation {
  id: string;
  /** Required by the dataset contract. Typed as optional/untightened here
   *  (rather than `EvalSplit`) because the value actually comes from
   *  `JSON.parse` on disk — a missing or misspelled split must be
   *  detectable at runtime and reported, not something the type system
   *  quietly coerces into "dev". */
  split?: string;
  note?: string;
  turns: EvalTurn[];
}

/** One traced turn from `trace.json` — the affect frame the real Rust model
 *  produced for this turn, minus `ts` (the TS side supplies `ts: 0`). */
export interface TraceTurn {
  index: number;
  speaker?: string | null;
  text: string;
  affect: Omit<AffectSnapshot, "ts">;
}

/** One conversation's worth of traced turns from `trace.json`. */
export interface TraceConversation {
  id: string;
  turns: TraceTurn[];
}

/** The shape of the whole `trace.json` file. */
export interface TraceFile {
  conversations: TraceConversation[];
}

/** A single scored turn where the prediction diverged from the label under
 *  strict scoring (regardless of whether lenient scoring accepts it). */
export interface Mismatch {
  id: string;
  index: number;
  text: string;
  expect: EmotionName;
  alsoOk: EmotionName[];
  predicted: EmotionName;
  why: string;
}

/** Recall/precision for one emotion across the whole scored set. `support`
 *  is how many gold turns are labelled this emotion; `predicted` is how many
 *  turns the model predicted this emotion for (strict). */
export interface EmotionStats {
  emotion: EmotionName;
  support: number;
  predicted: number;
  correct: number;
  recall: number;
  precision: number;
}

export interface EvalReport {
  turns: number;
  strictCorrect: number;
  lenientCorrect: number;
  strictAccuracy: number;
  lenientAccuracy: number;
  /** confusion.get(gold)!.get(predicted)! -> count. Every EMOTIONS x
   *  EMOTIONS cell exists, zero-filled, even when never observed. */
  confusion: Map<EmotionName, Map<EmotionName, number>>;
  perEmotion: EmotionStats[];
  mismatches: Mismatch[];
  /** Join/shape problems found while scoring: dataset ids with no matching
   *  trace, trace ids with no matching dataset entry, or a conversation
   *  whose turn counts disagree between the two files. Scoring for an
   *  affected conversation is skipped, not guessed at, so a non-empty
   *  `problems` means the accuracy numbers above are computed over fewer
   *  turns than `dataset` alone would suggest. */
  problems: string[];
  /** How many turns, summed across every scored conversation, the
   *  *predicted* emotion differed from the previous turn's predicted
   *  emotion (a conversation's first scored turn never counts — there is no
   *  previous turn to differ from). See `countSwitchesAndFlickers`. */
  switches: number;
  /** How many of those predicted switches reverted to the emotion they left
   *  behind within the next two turns — the direct measure of "chattering
   *  between two expressions" rather than genuinely changing its mind. See
   *  `countSwitchesAndFlickers` for the exact definition (an A→B→A run of
   *  three turns is exactly one flicker, not two). */
  flickers: number;
  /** Same as `switches`, but over the *gold* `expect` label sequence
   *  instead of the prediction. Exists so `switches` has something to be
   *  judged against: a predicted `switches` far below the gold count means
   *  the model is under-reacting (too sticky), far above means it's still
   *  chattering. */
  goldSwitches: number;
  /** Same as `flickers`, but over the gold `expect` label sequence. */
  goldFlickers: number;
}

/** Three `EvalReport`s over the same trace, sliced by `EvalConversation.split`
 *  (see `scoreDataset`), plus the numbers that only make sense once all
 *  three exist side by side. */
export interface MultiSplitReport {
  /** Every conversation with a valid `split`, dev and holdout together. */
  overall: EvalReport;
  dev: EvalReport;
  holdout: EvalReport;
  /** Strict accuracy of the trivial "always predict neutral" constant
   *  model, computed over the same scored turns as the matching report
   *  above (so it is directly comparable turn-for-turn, not just a global
   *  estimate). This is the floor: a tuned model that cannot beat guessing
   *  neutral every time is not doing anything. */
  constantNeutralStrictAccuracy: { overall: number; dev: number; holdout: number };
  /** `dev.strictAccuracy - holdout.strictAccuracy`. The direct overfitting
   *  signal `split` exists to produce: a large positive gap means whatever
   *  was tuned (affect.rs's keyword lists, vrm-emotion.ts's thresholds) fits
   *  dev's specific phrasings better than it generalizes to phrasings
   *  nobody was looking at while tuning. Near zero (or negative) is the
   *  healthy case. */
  devHoldoutStrictGap: number;
}

/**
 * Counts switches and flickers in one conversation's emotion sequence
 * (turn order, oldest first).
 *
 * A "switch" is any turn whose emotion differs from the immediately
 * preceding turn's; the first turn in a sequence can never switch, since it
 * has no preceding turn.
 *
 * A "flicker" is a switch that *reverts* — the emotion it left behind
 * reappears at either of the next two turns. This is scored per switch, not
 * per turn, so a A→B→A run of three turns is exactly **one** flicker: the
 * A→B switch (turn 1) reverts at turn 2, which is the very next turn. The
 * B→A switch that performs that reversion (turn 2) is itself just a switch
 * — with only three turns in this example there's nothing after it to check
 * for *its* own reversion, so it does not add a second flicker. A longer
 * chatter run (A→B→A→B→A→...) is scored the same way, one flicker per
 * switch that reverts within its own two-turn lookahead, so it is *not*
 * halved or otherwise specially collapsed — more back-and-forth still
 * produces a larger flicker count.
 */
export function countSwitchesAndFlickers(sequence: readonly EmotionName[]): { switches: number; flickers: number } {
  let switches = 0;
  let flickers = 0;
  for (let i = 1; i < sequence.length; i++) {
    if (sequence[i] === sequence[i - 1]) continue;
    switches++;
    const prior = sequence[i - 1];
    const revertsWithinTwoTurns =
      (i + 1 < sequence.length && sequence[i + 1] === prior) ||
      (i + 2 < sequence.length && sequence[i + 2] === prior);
    if (revertsWithinTwoTurns) flickers++;
  }
  return { switches, flickers };
}

function emptyConfusion(): Map<EmotionName, Map<EmotionName, number>> {
  const confusion = new Map<EmotionName, Map<EmotionName, number>>();
  for (const gold of EMOTIONS) {
    const row = new Map<EmotionName, number>();
    for (const predicted of EMOTIONS) row.set(predicted, 0);
    confusion.set(gold, row);
  }
  return confusion;
}

/**
 * Score a decoded `trace.json` against a decoded `dataset.jsonl` (already
 * split into one EvalConversation per line). Conversations are matched by
 * `id`; within a matched conversation, turns are matched positionally by
 * `index` (trace) against array position (dataset) — the contract requires
 * the two to already line up, so a length mismatch is a `problems` entry,
 * not something this function tries to realign.
 */
export function scoreTrace(dataset: EvalConversation[], trace: TraceConversation[]): EvalReport {
  const problems: string[] = [];
  const mismatches: Mismatch[] = [];
  const confusion = emptyConfusion();
  const support = new Map<EmotionName, number>(EMOTIONS.map((e) => [e, 0]));
  const predictedCount = new Map<EmotionName, number>(EMOTIONS.map((e) => [e, 0]));
  const correctCount = new Map<EmotionName, number>(EMOTIONS.map((e) => [e, 0]));

  const traceById = new Map<string, TraceConversation>();
  for (const conv of trace) {
    if (traceById.has(conv.id)) {
      problems.push(`trace.json has more than one conversation with id "${conv.id}"`);
      continue;
    }
    traceById.set(conv.id, conv);
  }

  const datasetIds = new Set(dataset.map((c) => c.id));
  for (const conv of dataset) {
    if (!traceById.has(conv.id)) {
      problems.push(`dataset id "${conv.id}" has no matching conversation in trace.json`);
    }
  }
  for (const conv of trace) {
    if (!datasetIds.has(conv.id)) {
      problems.push(`trace.json id "${conv.id}" has no matching conversation in dataset.jsonl`);
    }
  }

  let turns = 0;
  let strictCorrect = 0;
  let lenientCorrect = 0;
  let switches = 0;
  let flickers = 0;
  let goldSwitches = 0;
  let goldFlickers = 0;

  for (const conv of dataset) {
    const traced = traceById.get(conv.id);
    if (!traced) continue; // already recorded as a problem above

    if (traced.turns.length !== conv.turns.length) {
      problems.push(
        `conversation "${conv.id}" has ${conv.turns.length} dataset turn(s) but ${traced.turns.length} trace turn(s); skipping its turns`,
      );
      continue;
    }

    // Reset per conversation: each conversation is its own encounter, so
    // the hysteresis state (and the switch/flicker sequences it drives)
    // must not carry over from whatever the previous conversation ended on.
    let previous: EmotionDecision | null = null;
    const predictedSequence: EmotionName[] = [];
    const goldSequence: EmotionName[] = [];

    for (let index = 0; index < conv.turns.length; index++) {
      const goldTurn = conv.turns[index];
      const traceTurn = traced.turns.find((t) => t.index === index);
      if (!traceTurn) {
        problems.push(`conversation "${conv.id}" has no trace turn at index ${index}; skipping it`);
        continue;
      }

      const affect: AffectSnapshot = { ts: 0, ...traceTurn.affect };
      const decision = decideEmotion(affect, previous);
      previous = decision;
      const predicted = decision.emotion;
      const expect = goldTurn.expect;
      const alsoOk = goldTurn.alsoOk ?? [];

      predictedSequence.push(predicted);
      goldSequence.push(expect);

      turns++;
      support.set(expect, (support.get(expect) ?? 0) + 1);
      predictedCount.set(predicted, (predictedCount.get(predicted) ?? 0) + 1);
      confusion.get(expect)!.set(predicted, (confusion.get(expect)!.get(predicted) ?? 0) + 1);

      const strictHit = predicted === expect;
      const lenientHit = strictHit || alsoOk.includes(predicted);
      if (strictHit) {
        strictCorrect++;
        correctCount.set(expect, (correctCount.get(expect) ?? 0) + 1);
      }
      if (lenientHit) lenientCorrect++;

      if (!strictHit) {
        mismatches.push({
          id: conv.id,
          index,
          text: goldTurn.text,
          expect,
          alsoOk,
          predicted,
          why: goldTurn.why,
        });
      }
    }

    const predictedStats = countSwitchesAndFlickers(predictedSequence);
    switches += predictedStats.switches;
    flickers += predictedStats.flickers;
    const goldStats = countSwitchesAndFlickers(goldSequence);
    goldSwitches += goldStats.switches;
    goldFlickers += goldStats.flickers;
  }

  const perEmotion: EmotionStats[] = EMOTIONS.map((emotion) => {
    const s = support.get(emotion) ?? 0;
    const p = predictedCount.get(emotion) ?? 0;
    const c = correctCount.get(emotion) ?? 0;
    return {
      emotion,
      support: s,
      predicted: p,
      correct: c,
      recall: s === 0 ? 0 : c / s,
      precision: p === 0 ? 0 : c / p,
    };
  });

  return {
    turns,
    strictCorrect,
    lenientCorrect,
    strictAccuracy: turns === 0 ? 0 : strictCorrect / turns,
    lenientAccuracy: turns === 0 ? 0 : lenientCorrect / turns,
    confusion,
    perEmotion,
    mismatches,
    problems,
    switches,
    flickers,
    goldSwitches,
    goldFlickers,
  };
}

/** Strict accuracy of the constant "always predict neutral" model, computed
 *  over the exact turn set `report` was scored over (i.e. `report.turns`,
 *  the same denominator `report.strictAccuracy` uses) so the two are
 *  directly comparable. Reads straight off `perEmotion` rather than
 *  re-deriving anything: predicting "neutral" every time is correct exactly
 *  on the turns whose gold label is neutral, and `support` for the
 *  "neutral" row is already that count. */
function constantNeutralStrictAccuracy(report: EvalReport): number {
  if (report.turns === 0) return 0;
  const neutral = report.perEmotion.find((s) => s.emotion === "neutral");
  return (neutral?.support ?? 0) / report.turns;
}

/**
 * Scores a dataset three ways — overall (every conversation with a valid
 * `split`), dev-only, and holdout-only — by slicing both `dataset` and
 * `trace` down to matching conversation ids before delegating to
 * `scoreTrace` for each slice. Trace is sliced along with dataset (not just
 * dataset) specifically so a dev-only or holdout-only report never reports
 * spurious "trace id has no matching dataset conversation" problems for
 * conversations that simply belong to the *other* split — those trace
 * entries are still present in `trace`, just irrelevant to this slice.
 *
 * A conversation whose `split` is missing or not `"dev"`/`"holdout"` is
 * excluded from all three reports and recorded in `overall.problems` — it
 * is never silently folded into "dev", since that would make an
 * unlabelled conversation invisibly inflate the split that happens to be
 * the default.
 */
export function scoreDataset(dataset: EvalConversation[], trace: TraceConversation[]): MultiSplitReport {
  const splitProblems: string[] = [];
  const validDataset: EvalConversation[] = [];
  const excludedIds = new Set<string>();
  for (const conv of dataset) {
    if (conv.split !== "dev" && conv.split !== "holdout") {
      splitProblems.push(
        `dataset id "${conv.id}" has missing or invalid split ${JSON.stringify(
          conv.split ?? null,
        )} (must be "dev" or "holdout"); excluded from every report`,
      );
      excludedIds.add(conv.id);
      continue;
    }
    validDataset.push(conv);
  }

  // Drop the trace entries for conversations we just excluded for a bad
  // split, so they don't *also* show up as "trace id has no matching
  // dataset conversation" — that would be a second, misleading problem for
  // the same underlying cause. A trace id that never had a dataset
  // conversation at all (a genuine orphan) is untouched and still flagged.
  const overallTrace = trace.filter((t) => !excludedIds.has(t.id));
  const overall = scoreTrace(validDataset, overallTrace);
  overall.problems = [...splitProblems, ...overall.problems];

  const devDataset = validDataset.filter((c) => c.split === "dev");
  const holdoutDataset = validDataset.filter((c) => c.split === "holdout");
  const devIds = new Set(devDataset.map((c) => c.id));
  const holdoutIds = new Set(holdoutDataset.map((c) => c.id));
  const dev = scoreTrace(
    devDataset,
    trace.filter((t) => devIds.has(t.id)),
  );
  const holdout = scoreTrace(
    holdoutDataset,
    trace.filter((t) => holdoutIds.has(t.id)),
  );

  return {
    overall,
    dev,
    holdout,
    constantNeutralStrictAccuracy: {
      overall: constantNeutralStrictAccuracy(overall),
      dev: constantNeutralStrictAccuracy(dev),
      holdout: constantNeutralStrictAccuracy(holdout),
    },
    devHoldoutStrictGap: dev.strictAccuracy - holdout.strictAccuracy,
  };
}

function pct(n: number): string {
  return `${(n * 100).toFixed(1)}%`;
}

const MAX_MISMATCHES_SHOWN = 40;

/** Render a single EvalReport as a plain-text report readable in a
 *  terminal: headline accuracy, a gold x predicted confusion table,
 *  per-emotion recall/precision, and up to MAX_MISMATCHES_SHOWN individual
 *  mismatches (with an explicit "... and N more" if the list was cut).
 *
 *  This formats *one* report in isolation (no notion of dev/holdout) — see
 *  `formatReport` below for the three-way overall/dev/holdout view that
 *  callers should normally reach for. */
export function formatEvalReport(report: EvalReport): string {
  const lines: string[] = [];

  if (report.problems.length > 0) {
    lines.push(`PROBLEMS (${report.problems.length}) — join between dataset and trace is not clean:`);
    for (const p of report.problems) lines.push(`  ! ${p}`);
    lines.push("");
  }

  lines.push(`turns scored: ${report.turns}`);
  lines.push(
    `strict accuracy:  ${pct(report.strictAccuracy)}  (${report.strictCorrect}/${report.turns})`,
  );
  lines.push(
    `lenient accuracy: ${pct(report.lenientAccuracy)}  (${report.lenientCorrect}/${report.turns})`,
  );
  lines.push("");

  // switches/flickers measure chattering directly (accuracy alone can't
  // distinguish "same accuracy, but stopped flickering" from "same accuracy,
  // still flickering"). Gold's own counts are printed alongside so the
  // predicted numbers have something to be judged against, not just a bare
  // count with no reference point.
  lines.push(`switches (predicted): ${report.switches}   switches (gold): ${report.goldSwitches}`);
  lines.push(`flickers (predicted): ${report.flickers}   flickers (gold): ${report.goldFlickers}`);
  lines.push("");

  // Confusion matrix: rows are gold, columns are predicted.
  const colWidth = Math.max(9, ...EMOTIONS.map((e) => e.length + 1));
  const header = "gold\\pred".padEnd(colWidth) + EMOTIONS.map((e) => e.padStart(colWidth)).join("");
  lines.push(header);
  for (const gold of EMOTIONS) {
    const row = report.confusion.get(gold)!;
    const cells = EMOTIONS.map((pred) => String(row.get(pred) ?? 0).padStart(colWidth));
    lines.push(gold.padEnd(colWidth) + cells.join(""));
  }
  lines.push("");

  lines.push("per-emotion recall/precision:");
  const nameWidth = Math.max(...EMOTIONS.map((e) => e.length)) + 2;
  for (const stat of report.perEmotion) {
    lines.push(
      `  ${stat.emotion.padEnd(nameWidth)} support=${String(stat.support).padStart(3)}  predicted=${String(
        stat.predicted,
      ).padStart(3)}  recall=${pct(stat.recall).padStart(6)}  precision=${pct(stat.precision).padStart(6)}`,
    );
  }
  lines.push("");

  lines.push(`mismatches (strict): ${report.mismatches.length}`);
  const shown = report.mismatches.slice(0, MAX_MISMATCHES_SHOWN);
  for (const m of shown) {
    const alsoOk = m.alsoOk.length > 0 ? ` (alsoOk: ${m.alsoOk.join(", ")})` : "";
    lines.push(`  [${m.id}#${m.index}] "${m.text}"`);
    lines.push(`      expect=${m.expect}${alsoOk}  predicted=${m.predicted}`);
    lines.push(`      why: ${m.why}`);
  }
  if (report.mismatches.length > shown.length) {
    lines.push(`  ... and ${report.mismatches.length - shown.length} more`);
  }

  return lines.join("\n");
}

function formatSplitHeadline(label: string, report: EvalReport, constantAccuracy: number): string[] {
  return [
    `[${label}] turns=${report.turns}`,
    `  strict accuracy:                          ${pct(report.strictAccuracy)}  (${report.strictCorrect}/${report.turns})`,
    `  lenient accuracy:                          ${pct(report.lenientAccuracy)}  (${report.lenientCorrect}/${report.turns})`,
    `  constant "always neutral" strict accuracy: ${pct(constantAccuracy)}  <- floor a real model must clear`,
  ];
}

/** Render a `MultiSplitReport` (see `scoreDataset`) as a plain-text report:
 *  strict/lenient accuracy for overall, dev, and holdout side by side, each
 *  next to the constant "always neutral" baseline; the dev-holdout strict
 *  accuracy gap called out explicitly (the overfitting signal `split`
 *  exists to produce); and the full single-report detail — problems,
 *  switches/flickers, confusion matrix, per-emotion recall/precision, and
 *  capped mismatch list — for `overall`, via `formatEvalReport`. */
export function formatReport(multi: MultiSplitReport): string {
  const lines: string[] = [];

  lines.push(...formatSplitHeadline("overall", multi.overall, multi.constantNeutralStrictAccuracy.overall));
  lines.push("");
  lines.push(...formatSplitHeadline("dev", multi.dev, multi.constantNeutralStrictAccuracy.dev));
  lines.push("");
  lines.push(...formatSplitHeadline("holdout", multi.holdout, multi.constantNeutralStrictAccuracy.holdout));
  lines.push("");

  const gapPp = multi.devHoldoutStrictGap * 100;
  const gapSign = gapPp >= 0 ? "+" : "";
  lines.push(
    `dev - holdout strict accuracy gap: ${gapSign}${gapPp.toFixed(1)}pp` +
      `  (large positive => likely fit to dev's specific phrasing rather than generalizing)`,
  );
  lines.push("");
  lines.push("=== overall detail ===");
  lines.push(formatEvalReport(multi.overall));

  return lines.join("\n");
}
