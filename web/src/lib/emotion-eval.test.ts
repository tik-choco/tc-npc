// Tests for the emotion-eval scoring harness (see eval/emotion/CONTRACT.md
// for the full three-artifact picture). Three jobs:
//
//   1. Exercise scoreTrace()/formatReport() against small, hand-built
//      dataset/trace fixtures that never touch disk. These must always
//      pass — they lock down strict vs. lenient scoring, the confusion
//      matrix, and the "problems" detection for a join gone wrong (missing
//      id on either side, turn-count mismatch).
//   2. Exercise countSwitchesAndFlickers() directly against short,
//      hand-picked emotion sequences, and scoreTrace()'s aggregation of it
//      (both the predicted and the gold side) over a small dataset.
//   3. Run the real scorer over eval/emotion/dataset.jsonl and
//      eval/emotion/trace.json if both exist on disk. Those files are
//      owned by other work (a hand-labelled dataset and a `cargo run`
//      trace generator) that may not have landed yet, so this half skips
//      quietly rather than failing when either is missing. No accuracy
//      threshold is asserted here — there is no baseline yet to compare
//      against — only that the report comes back with zero `problems`
//      (i.e. the join between dataset and trace is clean, and every
//      conversation carries a valid `split`); the full three-way
//      overall/dev/holdout report is printed for a human to read.
//   4. Exercise scoreDataset() — the overall/dev/holdout split-aggregation
//      layer built on top of scoreTrace() — against small fixtures: a
//      conversation's turns must land in exactly one of dev/holdout (and
//      also count toward overall), a missing/invalid `split` must be
//      reported in `problems` and excluded from every report rather than
//      silently scored as dev, and the constant "always neutral" baseline
//      and the dev-holdout strict-accuracy gap must be computed correctly
//      off of whichever turns actually got scored.
//
// A note on fixture design for (1) and (2): decideEmotion (lib/vrm-emotion.ts)
// is a hysteresis-aware state machine — a turn's decision depends on the
// *previous* turn's decision within the same conversation, and the exact
// hysteresis formula (streak bonus, minimum hold, etc.) is tuned elsewhere
// and not something this file should assume the shape of. Two things keep
// these fixtures honest without depending on that tuning:
//
//   - Multi-turn scoreTrace fixtures here use affect frames that never
//     deviate from baseline (see `baselineAffect()`), so there is never any
//     signal for decideEmotion to act on and the predicted emotion has
//     nothing to do but stay "neutral" turn after turn, regardless of how
//     hysteresis is tuned.
//   - Fixtures that *do* need a non-neutral prediction (to test strict vs.
//     lenient scoring, confusion cells, recall/precision) use single-turn
//     conversations, so decideEmotion always sees `previous: null` — the
//     one case guaranteed not to carry any hysteresis bonus from a prior
//     turn — and use a large drive deviation so the predicted emotion is
//     unambiguous even if the underlying vote weights get retuned.
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { AffectSnapshot } from "../hooks/useNpcSocket";
import type { EmotionName } from "../vrm/animation";
import {
  countSwitchesAndFlickers,
  type EvalConversation,
  formatEvalReport,
  formatReport,
  scoreDataset,
  scoreTrace,
  type TraceConversation,
} from "./emotion-eval";

/** A minimal but complete affect frame (minus `ts`), at baseline so nothing
 *  fires unless a test explicitly deviates a drive. */
function baselineAffect(drives: AffectSnapshot["drives"] = []): TraceConversation["turns"][number]["affect"] {
  return {
    familiarity: 0,
    closing: false,
    inviteCaution: false,
    partner: null,
    partnerKnown: false,
    partnerSwitched: false,
    partnerAway: false,
    drives,
  };
}

/** A drive deviation large enough to win against any plausible hysteresis
 *  "stay where you are" bonus or vote-weight retuning, so single-turn
 *  fixtures (`previous: null`) get an unambiguous predicted emotion without
 *  this file needing to know decideEmotion's exact thresholds. */
function strongDeviation(key: string): AffectSnapshot["drives"] {
  return [{ key, level: 5.5, base: 0.5 }];
}

describe("scoreTrace", () => {
  it("scores strict and lenient separately: alsoOk saves a lenient miss but not a strict one", () => {
    const dataset: EvalConversation[] = [
      { id: "conv-a", turns: [{ text: "やあ", expect: "neutral", why: "何も起きていない" }] },
      // gaba deviation below will predict "relaxed"; expect is "happy" but
      // "relaxed" is alsoOk, so this is a lenient hit, strict miss. A
      // single-turn conversation of its own, so decideEmotion sees
      // `previous: null` — no hysteresis to reason about.
      {
        id: "conv-b",
        turns: [{ text: "落ち着くね", expect: "happy", alsoOk: ["relaxed"], why: "ラベル自体は微妙" }],
      },
    ];
    const trace: TraceConversation[] = [
      { id: "conv-a", turns: [{ index: 0, text: "やあ", affect: baselineAffect() }] },
      {
        id: "conv-b",
        turns: [{ index: 0, text: "落ち着くね", affect: baselineAffect(strongDeviation("gaba")) }],
      },
    ];

    const report = scoreTrace(dataset, trace);
    expect(report.problems).toEqual([]);
    expect(report.turns).toBe(2);
    expect(report.strictCorrect).toBe(1); // only the neutral turn
    expect(report.lenientCorrect).toBe(2); // relaxed is alsoOk for the second
    expect(report.strictAccuracy).toBeCloseTo(0.5);
    expect(report.lenientAccuracy).toBeCloseTo(1);

    expect(report.mismatches).toHaveLength(1);
    expect(report.mismatches[0]).toMatchObject({
      id: "conv-b",
      index: 0,
      expect: "happy",
      alsoOk: ["relaxed"],
      predicted: "relaxed",
    });
  });

  it("fills every cell of the gold x predicted confusion matrix, including zeros", () => {
    const dataset: EvalConversation[] = [
      { id: "c", turns: [{ text: "t", expect: "sad", why: "cortisol up" }] },
    ];
    const trace: TraceConversation[] = [
      { id: "c", turns: [{ index: 0, text: "t", affect: baselineAffect(strongDeviation("cortisol")) }] },
    ];
    const report = scoreTrace(dataset, trace);

    // All six expressions must be present as both rows and columns, even
    // though only sad/sad was ever observed.
    expect([...report.confusion.keys()].sort()).toEqual(
      ["angry", "happy", "neutral", "relaxed", "sad", "surprised"].sort(),
    );
    for (const row of report.confusion.values()) {
      expect(row.size).toBe(6);
    }
    expect(report.confusion.get("sad")!.get("sad")).toBe(1);
    expect(report.confusion.get("neutral")!.get("neutral")).toBe(0);
    expect(report.confusion.get("happy")!.get("angry")).toBe(0);
  });

  it("flags a dataset id with no matching trace conversation, and skips it rather than guessing", () => {
    const dataset: EvalConversation[] = [
      { id: "only-in-dataset", turns: [{ text: "t", expect: "neutral", why: "why" }] },
    ];
    const trace: TraceConversation[] = [];

    const report = scoreTrace(dataset, trace);
    expect(report.turns).toBe(0);
    expect(report.problems).toEqual([
      'dataset id "only-in-dataset" has no matching conversation in trace.json',
    ]);
  });

  it("flags a trace id with no matching dataset conversation", () => {
    const dataset: EvalConversation[] = [];
    const trace: TraceConversation[] = [
      { id: "only-in-trace", turns: [{ index: 0, text: "t", affect: baselineAffect() }] },
    ];

    const report = scoreTrace(dataset, trace);
    expect(report.problems).toEqual([
      'trace.json id "only-in-trace" has no matching conversation in dataset.jsonl',
    ]);
  });

  it("flags a turn-count mismatch within a matched conversation, and does not score any of its turns", () => {
    const dataset: EvalConversation[] = [
      {
        id: "conv-b",
        turns: [
          { text: "one", expect: "neutral", why: "why" },
          { text: "two", expect: "neutral", why: "why" },
        ],
      },
    ];
    const trace: TraceConversation[] = [
      { id: "conv-b", turns: [{ index: 0, text: "one", affect: baselineAffect() }] },
    ];

    const report = scoreTrace(dataset, trace);
    expect(report.turns).toBe(0);
    expect(report.problems).toEqual([
      'conversation "conv-b" has 2 dataset turn(s) but 1 trace turn(s); skipping its turns',
    ]);
  });

  it("computes per-emotion recall and precision independently", () => {
    // Two "happy" gold turns: one predicted happy (correct), one predicted
    // neutral (missed) -> recall 1/2. One "neutral" gold turn predicted
    // happy (a false positive for happy) -> precision 1/2 for happy. Each
    // turn is its own single-turn conversation so every prediction sees
    // `previous: null`, independent of the others.
    const dataset: EvalConversation[] = [
      { id: "c1", turns: [{ text: "a", expect: "happy", why: "w" }] },
      { id: "c2", turns: [{ text: "b", expect: "happy", why: "w" }] },
      { id: "c3", turns: [{ text: "c", expect: "neutral", why: "w" }] },
    ];
    const trace: TraceConversation[] = [
      { id: "c1", turns: [{ index: 0, text: "a", affect: baselineAffect(strongDeviation("oxytocin")) }] }, // happy
      { id: "c2", turns: [{ index: 0, text: "b", affect: baselineAffect() }] }, // neutral: recall miss
      { id: "c3", turns: [{ index: 0, text: "c", affect: baselineAffect(strongDeviation("oxytocin")) }] }, // happy: precision miss
    ];
    const report = scoreTrace(dataset, trace);
    const happy = report.perEmotion.find((s) => s.emotion === "happy")!;
    expect(happy.support).toBe(2);
    expect(happy.predicted).toBe(2);
    expect(happy.correct).toBe(1);
    expect(happy.recall).toBeCloseTo(0.5);
    expect(happy.precision).toBeCloseTo(0.5);
  });
});

describe("countSwitchesAndFlickers", () => {
  it("counts zero switches and zero flickers for a constant sequence", () => {
    const seq: EmotionName[] = ["neutral", "neutral", "neutral"];
    expect(countSwitchesAndFlickers(seq)).toEqual({ switches: 0, flickers: 0 });
  });

  it("counts switches with no reversion as switches but not flickers", () => {
    // neutral -> happy -> sad -> angry: each turn switches, but nothing
    // ever comes back to a prior emotion, so no flickers.
    const seq: EmotionName[] = ["neutral", "happy", "sad", "angry"];
    expect(countSwitchesAndFlickers(seq)).toEqual({ switches: 3, flickers: 0 });
  });

  it("counts an immediate A->B->A reversion as exactly one flicker", () => {
    // The neutral->happy switch (turn 1) reverts at the very next turn
    // (turn 2) -- one flicker. The happy->neutral switch that performs the
    // reversion (turn 2) has nothing after it in this 3-turn sequence to
    // revert to, so it stays a plain switch, not a second flicker.
    const seq: EmotionName[] = ["neutral", "happy", "neutral"];
    expect(countSwitchesAndFlickers(seq)).toEqual({ switches: 2, flickers: 1 });
  });

  it("counts a reversion two turns after the switch as a flicker too", () => {
    // neutral -> happy -> sad -> neutral: the neutral->happy switch (turn 1)
    // reverts to neutral at turn 3 -- two turns after the switch, still
    // inside the flicker window.
    const seq: EmotionName[] = ["neutral", "happy", "sad", "neutral"];
    expect(countSwitchesAndFlickers(seq)).toEqual({ switches: 3, flickers: 1 });
  });

  it("does not count a reversion three turns after the switch (outside the window)", () => {
    // neutral -> happy -> sad -> angry -> neutral: the neutral->happy switch
    // only reverts three turns later, past the two-turn flicker window, so
    // every switch here is counted but none is a flicker.
    const seq: EmotionName[] = ["neutral", "happy", "sad", "angry", "neutral"];
    expect(countSwitchesAndFlickers(seq)).toEqual({ switches: 4, flickers: 0 });
  });

  it("counts one flicker per reverting switch in a ping-pong run", () => {
    // neutral -> happy -> neutral -> happy -> neutral (A->B->A->B->A): all
    // four turn-to-turn switches happen (turns 1-4), and the first three
    // each revert at their very next turn, so three of the four are
    // flickers -- more back-and-forth yields a larger count, it isn't
    // collapsed to one. Only the last switch (turn 4, ...->neutral) has no
    // following turn to check for a reversion, so it stays a plain switch.
    const seq: EmotionName[] = ["neutral", "happy", "neutral", "happy", "neutral"];
    expect(countSwitchesAndFlickers(seq)).toEqual({ switches: 4, flickers: 3 });
  });
});

describe("scoreTrace switches/flickers aggregation", () => {
  it("sums gold switches/flickers to match countSwitchesAndFlickers over the same label sequence", () => {
    const dataset: EvalConversation[] = [
      {
        id: "c",
        turns: [
          { text: "t0", expect: "neutral", why: "w" },
          { text: "t1", expect: "happy", why: "w" },
          { text: "t2", expect: "neutral", why: "w" },
          { text: "t3", expect: "sad", why: "w" },
        ],
      },
    ];
    // All turns at baseline -- no drive deviates, so decideEmotion has
    // nothing to act on and the *predicted* sequence stays "neutral"
    // throughout, whatever the hysteresis tuning. Only gold switches here.
    const trace: TraceConversation[] = [
      {
        id: "c",
        turns: [0, 1, 2, 3].map((index) => ({ index, text: `t${index}`, affect: baselineAffect() })),
      },
    ];

    const report = scoreTrace(dataset, trace);
    expect(report.problems).toEqual([]);

    const goldExpected = countSwitchesAndFlickers(["neutral", "happy", "neutral", "sad"]);
    expect(goldExpected).toEqual({ switches: 3, flickers: 1 });
    expect(report.goldSwitches).toBe(goldExpected.switches);
    expect(report.goldFlickers).toBe(goldExpected.flickers);

    // Baseline throughout -> nothing for decideEmotion to switch to.
    expect(report.switches).toBe(0);
    expect(report.flickers).toBe(0);
  });

  it("does not carry switches/flickers across a conversation boundary", () => {
    // Single-turn conversations can never switch on their own (there is no
    // prior turn *within the same conversation* to differ from). If gold
    // switch/flicker counting leaked `previous` across conversations, c2's
    // lone turn would wrongly register as "switching" from c1's.
    const dataset: EvalConversation[] = [
      { id: "c1", turns: [{ text: "a", expect: "happy", why: "w" }] },
      { id: "c2", turns: [{ text: "b", expect: "sad", why: "w" }] },
    ];
    const trace: TraceConversation[] = [
      { id: "c1", turns: [{ index: 0, text: "a", affect: baselineAffect() }] },
      { id: "c2", turns: [{ index: 0, text: "b", affect: baselineAffect() }] },
    ];

    const report = scoreTrace(dataset, trace);
    expect(report.goldSwitches).toBe(0);
    expect(report.goldFlickers).toBe(0);
    expect(report.switches).toBe(0);
    expect(report.flickers).toBe(0);
  });
});

describe("formatEvalReport", () => {
  it("renders problems, headline accuracy, switches/flickers, the confusion table, and mismatches without throwing", () => {
    const dataset: EvalConversation[] = [
      { id: "c", turns: [{ text: "hello there", expect: "happy", why: "reward drive up" }] },
    ];
    const trace: TraceConversation[] = [
      { id: "c", turns: [{ index: 0, text: "hello there", affect: baselineAffect() }] }, // predicts neutral: strict miss
    ];
    const report = scoreTrace(dataset, trace);
    const text = formatEvalReport(report);

    expect(text).toContain("strict accuracy");
    expect(text).toContain("lenient accuracy");
    expect(text).toContain("switches (predicted)");
    expect(text).toContain("switches (gold)");
    expect(text).toContain("flickers (predicted)");
    expect(text).toContain("flickers (gold)");
    expect(text).toContain("gold\\pred");
    expect(text).toContain("mismatches (strict): 1");
    expect(text).toContain("hello there");
    expect(text).toContain("why: reward drive up");
  });

  it("caps the printed mismatch list and says how many more were cut", () => {
    const turns = Array.from({ length: 45 }, (_, i) => ({
      text: `turn ${i}`,
      expect: "happy" as const,
      why: "forced mismatch",
    }));
    const dataset: EvalConversation[] = [{ id: "c", turns }];
    const trace: TraceConversation[] = [
      {
        id: "c",
        turns: turns.map((t, index) => ({ index, text: t.text, affect: baselineAffect() })), // all predict neutral
      },
    ];
    const report = scoreTrace(dataset, trace);
    expect(report.mismatches).toHaveLength(45);

    const text = formatEvalReport(report);
    expect(text).toContain("... and 5 more");
  });
});

describe("scoreDataset", () => {
  it("splits scoring into overall/dev/holdout, each conversation landing in overall plus exactly one of dev/holdout", () => {
    const dataset: EvalConversation[] = [
      { id: "d1", split: "dev", turns: [{ text: "d1", expect: "happy", why: "w" }] },
      { id: "d2", split: "dev", turns: [{ text: "d2", expect: "neutral", why: "w" }] },
      { id: "h1", split: "holdout", turns: [{ text: "h1", expect: "sad", why: "w" }] },
    ];
    const trace: TraceConversation[] = [
      { id: "d1", turns: [{ index: 0, text: "d1", affect: baselineAffect(strongDeviation("oxytocin")) }] }, // happy
      { id: "d2", turns: [{ index: 0, text: "d2", affect: baselineAffect() }] }, // neutral
      { id: "h1", turns: [{ index: 0, text: "h1", affect: baselineAffect(strongDeviation("cortisol")) }] }, // sad
    ];

    const multi = scoreDataset(dataset, trace);
    expect(multi.overall.problems).toEqual([]);
    expect(multi.overall.turns).toBe(3);
    expect(multi.dev.turns).toBe(2);
    expect(multi.holdout.turns).toBe(1);

    // All three predictions were strict hits, so every slice is 100%.
    expect(multi.overall.strictAccuracy).toBeCloseTo(1);
    expect(multi.dev.strictAccuracy).toBeCloseTo(1);
    expect(multi.holdout.strictAccuracy).toBeCloseTo(1);

    // dev/holdout trace filtering must not leak cross-split "unmatched"
    // problems: scoring holdout alone must not complain that d1/d2 (dev
    // trace ids) have no matching dataset conversation.
    expect(multi.dev.problems).toEqual([]);
    expect(multi.holdout.problems).toEqual([]);
  });

  it("excludes a conversation with a missing split from every report and records it in problems, never defaulting it to dev", () => {
    const dataset: EvalConversation[] = [
      { id: "no-split", turns: [{ text: "t", expect: "neutral", why: "w" }] }, // split omitted entirely
      { id: "dev-one", split: "dev", turns: [{ text: "t2", expect: "neutral", why: "w" }] },
    ];
    const trace: TraceConversation[] = [
      { id: "no-split", turns: [{ index: 0, text: "t", affect: baselineAffect() }] },
      { id: "dev-one", turns: [{ index: 0, text: "t2", affect: baselineAffect() }] },
    ];

    const multi = scoreDataset(dataset, trace);
    // Excluded from every report's turn count...
    expect(multi.overall.turns).toBe(1);
    expect(multi.dev.turns).toBe(1);
    expect(multi.holdout.turns).toBe(0);
    // ...and specifically not folded into dev (which would make overall
    // and dev turn counts equal at 2).
    expect(multi.overall.problems).toHaveLength(1);
    expect(multi.overall.problems[0]).toContain("no-split");
    expect(multi.overall.problems[0]).toContain("missing or invalid split");
  });

  it("also flags an invalid (non dev/holdout) split value, not just a missing one", () => {
    const dataset: EvalConversation[] = [
      { id: "bad-split", split: "staging", turns: [{ text: "t", expect: "neutral", why: "w" }] },
    ];
    const trace: TraceConversation[] = [
      { id: "bad-split", turns: [{ index: 0, text: "t", affect: baselineAffect() }] },
    ];
    const multi = scoreDataset(dataset, trace);
    expect(multi.overall.turns).toBe(0);
    expect(multi.overall.problems).toHaveLength(1);
    expect(multi.overall.problems[0]).toContain('"staging"');
  });

  it("computes the constant always-neutral baseline per split, off the same scored-turn denominator as strictAccuracy", () => {
    const dataset: EvalConversation[] = [
      { id: "d1", split: "dev", turns: [{ text: "a", expect: "neutral", why: "w" }] },
      { id: "d2", split: "dev", turns: [{ text: "b", expect: "happy", why: "w" }] },
      { id: "h1", split: "holdout", turns: [{ text: "c", expect: "neutral", why: "w" }] },
      { id: "h2", split: "holdout", turns: [{ text: "d", expect: "neutral", why: "w" }] },
      { id: "h3", split: "holdout", turns: [{ text: "e", expect: "sad", why: "w" }] },
    ];
    const trace: TraceConversation[] = [
      { id: "d1", turns: [{ index: 0, text: "a", affect: baselineAffect() }] },
      { id: "d2", turns: [{ index: 0, text: "b", affect: baselineAffect() }] }, // model predicts neutral: strict miss
      { id: "h1", turns: [{ index: 0, text: "c", affect: baselineAffect() }] },
      { id: "h2", turns: [{ index: 0, text: "d", affect: baselineAffect() }] },
      { id: "h3", turns: [{ index: 0, text: "e", affect: baselineAffect() }] }, // model predicts neutral: strict miss
    ];

    const multi = scoreDataset(dataset, trace);
    // dev: 1/2 turns are gold-neutral -> constant baseline 0.5.
    expect(multi.constantNeutralStrictAccuracy.dev).toBeCloseTo(0.5);
    // holdout: 2/3 turns are gold-neutral -> constant baseline 2/3.
    expect(multi.constantNeutralStrictAccuracy.holdout).toBeCloseTo(2 / 3);
    // overall: 3/5 turns are gold-neutral -> constant baseline 0.6.
    expect(multi.constantNeutralStrictAccuracy.overall).toBeCloseTo(0.6);

    // gap = dev.strictAccuracy - holdout.strictAccuracy, both from the
    // actual (baseline-always-predicting) model above.
    expect(multi.devHoldoutStrictGap).toBeCloseTo(multi.dev.strictAccuracy - multi.holdout.strictAccuracy);
  });
});

describe("formatReport (multi-split)", () => {
  it("renders overall/dev/holdout headlines, the dev-holdout gap, and the overall detail without throwing", () => {
    const dataset: EvalConversation[] = [
      { id: "d1", split: "dev", turns: [{ text: "hello", expect: "happy", why: "reward drive up" }] },
      { id: "h1", split: "holdout", turns: [{ text: "world", expect: "sad", why: "cortisol up" }] },
    ];
    const trace: TraceConversation[] = [
      { id: "d1", turns: [{ index: 0, text: "hello", affect: baselineAffect() }] }, // predicts neutral: miss
      { id: "h1", turns: [{ index: 0, text: "world", affect: baselineAffect(strongDeviation("cortisol")) }] }, // hit
    ];

    const multi = scoreDataset(dataset, trace);
    const text = formatReport(multi);

    expect(text).toContain("[overall]");
    expect(text).toContain("[dev]");
    expect(text).toContain("[holdout]");
    expect(text).toContain("always neutral");
    expect(text).toContain("dev - holdout strict accuracy gap");
    expect(text).toContain("=== overall detail ===");
    expect(text).toContain("gold\\pred");
  });
});

// --- real-data run -----------------------------------------------------
//
// eval/emotion/dataset.jsonl and eval/emotion/trace.json are produced by
// separate pieces of work (a hand-labelled dataset, and a `cargo run -p
// npc-talk --example affect_trace` generator). Resolve paths from this
// file's own location, not the process cwd, since vitest here runs with
// cwd=web/.
const here = path.dirname(fileURLToPath(import.meta.url)); // web/src/lib
const repoRoot = path.resolve(here, "../../.."); // -> web/src -> web -> repo root
const datasetPath = path.join(repoRoot, "eval", "emotion", "dataset.jsonl");
const tracePath = path.join(repoRoot, "eval", "emotion", "trace.json");
const haveRealData = existsSync(datasetPath) && existsSync(tracePath);

function loadDataset(): EvalConversation[] {
  const text = readFileSync(datasetPath, "utf8");
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as EvalConversation);
}

function loadTrace(): TraceConversation[] {
  const text = readFileSync(tracePath, "utf8");
  const parsed = JSON.parse(text) as { conversations: TraceConversation[] };
  return parsed.conversations;
}

describe("real-data evaluation run", () => {
  // Skips quietly (rather than failing) when either input hasn't been
  // generated yet — this file only owns the scorer, not the dataset or the
  // trace generator.
  it.skipIf(!haveRealData)(
    "scores eval/emotion/dataset.jsonl against eval/emotion/trace.json with a clean join and every conversation split",
    () => {
      const dataset = loadDataset();
      const trace = loadTrace();
      const multi = scoreDataset(dataset, trace);

      // eslint-disable-next-line no-console
      console.log(formatReport(multi));

      // Dataset and trace must actually line up, and every conversation must
      // carry a valid split; a non-empty `problems` means the numbers above
      // can't be trusted.
      expect(multi.overall.problems).toEqual([]);
      // Both slices must be non-empty for the dev-holdout gap to mean
      // anything — a split with zero scored turns would make the gap a
      // vacuous 0-minus-0 or NaN rather than a real signal.
      expect(multi.dev.turns).toBeGreaterThan(0);
      expect(multi.holdout.turns).toBeGreaterThan(0);

      // The floors below are ratchets, not targets. They sit a little under
      // what the pipeline measured when this harness was built, so ordinary
      // re-tuning noise (regenerating the trace shifts the numbers by a point
      // or so either way) doesn't fail the suite, but a change that genuinely
      // undoes the work does. Raise them when a change earns it; never lower
      // one to make a red test green.
      //
      // Measured at the time of writing: overall strict 49.0% / lenient
      // 68.4%, holdout strict 47.5%.
      expect(multi.overall.strictAccuracy).toBeGreaterThan(0.45);
      expect(multi.overall.lenientAccuracy).toBeGreaterThan(0.63);

      // The real bar. "Always predict neutral" scores 41.3% overall purely
      // because most conversational turns genuinely are neutral, and the
      // original vote-based mapping came in *below* that constant model. Any
      // mapping worth having must beat it by a clear margin on both slices —
      // an accuracy that merely tracks the neutral share means the face has
      // stopped expressing anything.
      expect(multi.overall.strictAccuracy).toBeGreaterThan(multi.constantNeutralStrictAccuracy.overall + 0.05);
      expect(multi.holdout.strictAccuracy).toBeGreaterThan(multi.constantNeutralStrictAccuracy.holdout + 0.05);

      // Overfitting guard. Vocabulary lists and thresholds get tuned by
      // people who can read dev's `text` values, so dev beating holdout by a
      // wide margin is the signature of fitting those exact phrasings rather
      // than generalising. Tuning that tripped this during development was
      // reverted rather than kept (see affect.rs's pain-delta comment), so
      // the bound is a live constraint, not decoration.
      expect(multi.devHoldoutStrictGap).toBeLessThan(0.05);

      // A face that never moves reads as broken even when its accuracy is
      // fine, and one that flips every turn reads as noise. `switches` is
      // still well under gold's, so the floor is deliberately low — it only
      // catches a regression back to the near-frozen face the hysteresis
      // work started from. `flickers` (a switch reverted within two turns) is
      // capped against gold's own count: the labels themselves contain real
      // there-and-back changes, so the target is "no worse than a human
      // labeller", not zero.
      expect(multi.overall.switches).toBeGreaterThan(35);
      expect(multi.overall.flickers).toBeLessThanOrEqual(multi.overall.goldFlickers);
    },
  );
});
