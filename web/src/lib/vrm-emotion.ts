// Turning the affect model into a face.
//
// tc-town classifies each reply's emotion with a separate LLM call. tc-npc
// doesn't need to: it already computes a 22-drive affect state every turn
// (npc-talk's `PartnerAffect`, arriving as the `affect` frame), which is a
// richer and cheaper signal than asking a model to label the text. This maps
// that state onto the six VRM standard expressions the animator can drive.
//
// This is the second version of the mapping. The first (see git history,
// `emotionFromAffect`) was a stateless per-frame argmax over raw
// `level - base` deviations, and measured at 24.4% strict accuracy against
// eval/emotion/dataset.jsonl — worse than a constant "always predict
// neutral" model (48.75%). The diagnosis (see the wave-2 planning doc this
// change shipped with) found four structural problems, each addressed by a
// piece of this file rather than by nudging the old constants:
//
//   1. Neutral was a leftover ("nothing cleared the bar") instead of a
//      state that can win on its own merits, even though roughly half of
//      ordinary conversation should read as neutral. `computeScores` below
//      gives neutral an explicit score that falls as the other five rise,
//      so it competes in the same argmax instead of being a fallback.
//   2. Raw deviations favoured whichever drive had the most headroom: a
//      drive resting at base 0.2 can rise 0.8 before clamping, one resting
//      at 0.6 can only rise 0.4. `normalizedDeviation` rescales every
//      up-move by `1 - base` and every down-move by `base`, so "how much of
//      this drive's *available range* did the turn actually use" is
//      comparable across drives.
//   3. Several of the heaviest-voting drives in the old table (dopamine,
//      acetylcholine, orexin, histamine) fire mainly off sentence length and
//      question marks (see affect.rs's `char_count >= 16`, `novelty`,
//      `question` conditions) — arousal, not valence — and the diagnosis
//      traced most of the "everything becomes happy/surprised" failure to
//      them outvoting the actual valence drives. They're demoted to
//      low-weight modifiers here (see PRIMARY_VOTES vs. MODIFIER_VOTES)
//      that can only nudge a score that a real valence drive already put in
//      motion, never originate one on their own.
//   4. `wariness` (affect.rs's unfamiliarity penalty) pushes cortisol and
//      noradrenaline up for every turn with a near-stranger regardless of
//      what was actually said, which read as "polite greeting -> angry
//      face" in practice. `warinessBias` estimates and subtracts that
//      contribution from those two drives before scoring (see its own doc
//      comment for why this is an approximation and why that's fine here).
//
// On top of the per-frame score there is now history: `decideEmotion` takes
// the previous decision and applies hysteresis (a margin the challenger must
// clear, and a minimum number of turns the current expression is held)
// before switching, which is what turns "the scores happened to cross for
// one ambiguous turn" into "the face actually changes" — see its doc
// comment for the constants.
//
// Re-tuning round (against a 45-conversation/288-turn extended dataset):
// `sad` was reading as functionally dead (dev recall 9.1%, essentially never
// predicted) even though `angry`/`surprised` were healthy. The prime
// suspect was point 4 above: cortisol is `sad`'s heaviest primary vote (see
// PRIMARY_VOTES), and `warinessBias` was subtracting its *full* estimated
// wariness contribution from cortisol before scoring. Measured, not assumed
// (see CORTISOL_WARINESS_CORRECTION's doc comment): a full subtraction
// over-corrects and drags dev strict accuracy back down to the
// constant-neutral floor, because most of a stranger-conversation's early
// turns carry both wariness *and* some genuine cortisol-worthy content, and
// the full correction erases both together. Subtracting only 70% of
// cortisol's estimated wariness (i.e. leaving 30% of the estimate
// uncorrected) is what actually recovered `sad` without doing that —
// noradrenaline (a modifier, not a primary vote — see MODIFIER_VOTES) keeps
// the full subtraction, since over-correcting a modifier can't kill an
// emotion outright the way over-correcting a primary vote can. Separately,
// SWITCH_MARGIN came down from 0.15 to 0.13: with `sad` no longer starved,
// more turns had a real non-neutral challenger to offer, and the tighter
// margin let more of them through without pushing flicker count anywhere
// near gold's.
import type { AffectSnapshot } from "../hooks/useNpcSocket";
import type { EmotionName } from "../vrm/animation";
import { DEVIATION_THRESHOLD } from "./affect";

const EPS = 1e-6;

/** Emotion keys in a fixed order, used for score-map construction and
 *  argmax iteration. */
const EMOTIONS: readonly EmotionName[] = ["neutral", "happy", "angry", "sad", "relaxed", "surprised"];
const NON_NEUTRAL_EMOTIONS: readonly Exclude<EmotionName, "neutral">[] = [
  "happy",
  "angry",
  "sad",
  "relaxed",
  "surprised",
];

/**
 * How much of a drive's *available headroom* a turn's deviation used, in
 * `[0, 1]`. An "up" deviation is scaled by the room between base and the 1.0
 * ceiling (`1 - base`); a "down" deviation is scaled by the room between
 * base and the 0.0 floor (`base` itself). This is what makes a drive with a
 * low base (e.g. adrenaline at 0.2, room to rise 0.8) and a drive with a
 * high base (e.g. serotonin at 0.6, room to rise only 0.4) comparable: both
 * read as "1.0" only once they've used the *entirety* of their own range,
 * not some drive-independent absolute amount.
 */
function normalizedDeviation(level: number, base: number, direction: "up" | "down"): number {
  const delta = level - base;
  if (direction === "up") {
    if (delta <= 0) return 0;
    return delta / Math.max(1 - base, EPS);
  }
  if (delta >= 0) return 0;
  return -delta / Math.max(base, EPS);
}

/**
 * Estimated cortisol/noradrenaline contribution from affect.rs's wariness
 * term this turn: `wariness = max(0, 0.45 - familiarity)`, folded in as
 * `cortisol += wariness * 0.5` and `noradrenaline += wariness * 0.5` (see
 * `AffectState::update`). Subtracting this from those two drives' deviation
 * before scoring stops "I don't know you yet" from reading as anger or
 * sadness on its own.
 *
 * This is an approximation, not a reconstruction: affect.rs adds the
 * wariness delta straight into that turn's `level` *before* the drive's own
 * exponential decay is applied to next turn's *previous* level, so a run of
 * high-wariness turns leaves a residue that lingers a little after
 * familiarity climbs past 0.45 and wariness itself hits zero. Reproducing
 * that exactly would mean replaying the whole conversation inside a
 * supposedly-pure per-frame function, which defeats the point of this being
 * a pure function at all. We accept the approximation because both drives
 * are in affect.rs's "fast" decay tier (pull 0.2, the fastest of the three
 * tiers) — most of any given turn's wariness contribution is gone again
 * within a turn or two, so subtracting only the current turn's estimate
 * removes the dominant term even though it isn't exact.
 */
function warinessBias(affect: AffectSnapshot): number {
  // affect.familiarity is this turn's value *after* AffectState::update has
  // already run (0.1 per turn, +0.1 more if the turn's text matched a
  // bonding word) — but the wariness affect.rs actually applied to this
  // turn's cortisol/noradrenaline was computed from familiarity *before*
  // that increment. A single snapshot can't tell us which of the two
  // possible increments (0.1 or 0.2) happened, so we back out the midpoint
  // (0.15) as a best-effort estimate of the pre-update value rather than
  // using the post-update one outright, which would under-correct every
  // early turn (wariness looks smaller than it actually was).
  const estimatedPriorFamiliarity = Math.max(0, affect.familiarity - 0.15);
  return Math.max(0, 0.45 - estimatedPriorFamiliarity) * 0.5;
}

/**
 * How much of `warinessBias`'s estimate to actually subtract from cortisol,
 * as opposed to noradrenaline (which keeps the full 1.0). Measured against
 * eval/emotion/dataset.jsonl's dev split, holding everything else fixed:
 *
 *   1.0 (subtract the full estimate, the original behaviour): dev strict
 *       accuracy fell to 42.9% — exactly the constant "always neutral"
 *       floor for that split — and `sad` recall barely moved (9.1%). Most
 *       of an early-conversation turn's cortisol reading *is* wariness by
 *       this estimate, so subtracting all of it also erases whatever
 *       genuine sad-worthy content arrived the same turn; the eval set's
 *       early turns are disproportionately first-meetings, so this shows up
 *       as a broad accuracy collapse, not just a `sad`-shaped hole.
 *   0.0 (no subtraction at all): `sad` recall jumped past 25%, but dev
 *       strict accuracy *also* fell back to the constant-neutral floor —
 *       now via the opposite failure, several genuinely-neutral early
 *       "stranger" turns (a bare greeting, no distressing content) reading
 *       as `sad` off wariness alone, the exact failure this correction
 *       exists to prevent.
 *   0.7 (subtract 70%, i.e. leave 30% of the wariness estimate uncorrected
 *       in cortisol's reading): the measured sweet spot — dev
 *       strict accuracy rose to 49.7% (above both the 47.6% pre-retuning
 *       baseline and the 42.9% constant floor), `sad` recall reached 30%,
 *       and overall `neutral` recall held at 61.3%, clear of the 60% floor
 *       a real conversational partner needs. Values checked in 0.05-0.1
 *       steps around this one were all worse on dev strict accuracy than
 *       0.7 — not a round number chosen for its own sake.
 *
 * Applied only to cortisol, not noradrenaline: noradrenaline is a
 * MODIFIER_VOTES entry (it can only sharpen an `angry` score a primary vote
 * already started, never originate one — see that table's doc comment), so
 * over-correcting it can blunt `angry` a little but can never erase it the
 * way over-correcting cortisol — `sad`'s heaviest primary vote — erases
 * `sad`. Retested at 0.7 for noradrenaline too: dev strict accuracy did not
 * improve and the dev-holdout gap widened (+3.3pp vs. +2.3pp at 1.0), so
 * noradrenaline keeps the original full subtraction.
 */
const CORTISOL_WARINESS_CORRECTION = 0.7;

/**
 * Same kind of correction as `warinessBias`, for a second affect.rs
 * mechanism that inflates the same drives for a reason that isn't the
 * turn's emotional content: while `invite_caution_turns > 0` — a LURE-word
 * turn ("一緒に", "ついてきて", ...) from someone not yet familiar (see
 * `AffectState::update`) — affect.rs adds a flat `cortisol += 0.25`,
 * `cck += 0.2`, `noradrenaline += 0.15` on top of everything else, every
 * turn caution stays active (it decays over ~3 turns once nothing
 * re-triggers it). Unlike wariness this is exact rather than estimated
 * (`affect.inviteCaution` is the precise boolean, no reconstruction of a
 * pre-update value needed) — but *how much* of the current elevated level
 * is this turn's fresh +0.25/+0.2/+0.15 versus a previous turn's
 * contribution that hasn't fully decayed yet is still approximate, for the
 * same reason described in `warinessBias`'s doc comment: this is a pure
 * per-frame function with no access to the turns before the one it's
 * scoring. Sized at the flat per-turn constant itself, which is deliberately
 * a conservative (under- rather than over-) correction.
 */
function inviteCautionBias(key: string): number {
  if (key === "cortisol") return 0.25;
  if (key === "cck") return 0.2;
  if (key === "noradrenaline") return 0.15;
  return 0;
}

/** A drive's normalized deviation for one candidate vote, with the
 *  wariness/invite-caution corrections applied to the handful of drives
 *  those mechanisms touch. */
function voteDeviation(drive: { level: number; base: number }, key: string, direction: "up" | "down", affect: AffectSnapshot): number {
  let level = drive.level;
  if (direction === "up" && key === "cortisol") {
    // See CORTISOL_WARINESS_CORRECTION's doc comment: cortisol only gets a
    // partial correction, unlike noradrenaline's full one below.
    level -= warinessBias(affect) * CORTISOL_WARINESS_CORRECTION;
  }
  if (direction === "up" && key === "noradrenaline") {
    level -= warinessBias(affect);
  }
  if (direction === "up" && affect.inviteCaution && (key === "cortisol" || key === "cck" || key === "noradrenaline")) {
    level -= inviteCautionBias(key);
  }
  return normalizedDeviation(level, drive.base, direction);
}

interface Vote {
  readonly key: string;
  readonly direction: "up" | "down";
  readonly emotion: Exclude<EmotionName, "neutral">;
  readonly weight: number;
  /** Normalized-deviation floor this vote needs to clear before it counts
   *  at all. Defaults to DEVIATION_FLOOR when omitted; see individual votes
   *  below for why a handful override it. */
  readonly floor?: number;
}

/**
 * Normalized-deviation floor for an ordinary vote: a drive must have used
 * at least this fraction of its available range before it votes at all.
 * Reuses `DEVIATION_THRESHOLD` (0.2) from lib/affect.ts — that constant
 * already means "moved enough to matter" for the persona prompt's salience
 * cut, just applied there to a raw (non-normalized) delta. Normalizing
 * first (point 2 of the module doc comment) is what lets the same number
 * keep meaning "used a fifth of this drive's own room to move" for every
 * drive, instead of being generous to low-base drives and stingy to
 * high-base ones.
 */
const DEVIATION_FLOOR = DEVIATION_THRESHOLD;

/** Floor for the surprised votes below: much higher than DEVIATION_FLOOR,
 *  deliberately, so an arousal drive has to be close to saturated before it
 *  can read as surprise on its own — see those votes' comment for why this
 *  is still the right value even though the reason it's needed has changed
 *  (affect.rs gained dedicated surprise-word triggers on these same three
 *  drives; the floor's job now is suppressing sentence-shape false
 *  positives, not standing in for a missing signal). */
const SURPRISED_FLOOR = 0.5;

/**
 * Drives that actually carry emotional valence, per the drive semantics in
 * affect.rs's `AffectState::update`: what makes each one move is content
 * (positive/humor/bond words for happy, conflict/pain/urgency for
 * sad/angry, calm words for relaxed), not merely the shape of the
 * utterance. These originate a score on their own — clearing
 * `DEVIATION_FLOOR` is enough to put points on the board.
 */
const PRIMARY_VOTES: readonly Vote[] = [
  // happy: reward/bonding/warmth drives, all triggered by actually positive
  // content (humor, positive words, bond words, recovery/positive). A mild
  // "positive word present, no humor" turn produces a small delta on these
  // (endorphin +0.05, dhea +0.12) that often doesn't clear DEVIATION_FLOOR
  // even normalized — checked against eval/emotion/trace.json, lowering
  // their floor to let those mild-but-genuine turns in cost more precision
  // elsewhere (mostly neutral turns tipping into happy) than it gained in
  // happy recall, so the general floor stands: a mild happy turn reading as
  // neutral is the correct trade here, not a bug to chase away.
  { key: "endorphin", direction: "up", emotion: "happy", weight: 1.0 },
  { key: "anandamide", direction: "up", emotion: "happy", weight: 0.8 },
  // oxytocin gets a much higher floor than the rest of this bucket because
  // its rise isn't purely content-triggered the way the others are: on top
  // of bond words, affect.rs also ramps it a flat `+0.04 * min(own_turns,4)
  // / 4` every single turn (see AffectState::update), and its low base
  // (0.3) means that slow, content-independent ramp alone clears a normal
  // floor by turn 2-3 of *any* conversation regardless of what is being
  // said (checked against eval/emotion/trace.json: several "still just
  // making polite conversation" turns read as happy from this ramp alone).
  // 0.35 asks for more than the ramp can produce by itself within the
  // first few turns, so oxytocin only fires early when an actual bond word
  // adds on top of it — later in a long, genuinely warm conversation the
  // ramp plus real content still gets there.
  { key: "oxytocin", direction: "up", emotion: "happy", weight: 1.0, floor: 0.35 },
  { key: "dhea", direction: "up", emotion: "happy", weight: 0.6 },

  // sad: distress/pain drives, plus serotonin dropping below its own
  // baseline (only conflict words pull it down in affect.rs). cortisol is
  // weighted down (0.7, versus dynorphin's 0.9 for the same conflict/pain
  // trigger) because it is also the drive `voteDeviation` corrects for
  // wariness/invite-caution — on an early-conversation turn that is
  // genuinely sad *and* still low-familiarity, those corrections can remove
  // most of its real signal along with the confound, so dynorphin (which
  // nothing else touches) is treated as the more trustworthy of the two.
  { key: "cortisol", direction: "up", emotion: "sad", weight: 0.7 },
  { key: "dynorphin", direction: "up", emotion: "sad", weight: 0.9 },
  { key: "substance_p", direction: "up", emotion: "sad", weight: 0.7 },
  { key: "serotonin", direction: "down", emotion: "sad", weight: 0.8 },

  // angry: confrontation drives. Note noradrenaline is deliberately absent
  // here (see MODIFIER_VOTES) — in affect.rs it rises for any question or
  // urgent phrase, not specifically for conflict. adrenaline only fires on
  // `urgent || (conflict && question)` — a plain, non-urgent, non-question
  // conflict statement ("最悪だよ、あんな言い方しなくてもいいじゃん") never
  // moves it at all, so most of the eval set's angry turns depend on cck
  // alone. cck's own affect.rs delta for that same trigger (conflict||pain,
  // +0.13) is smaller than cortisol's (+0.18) or dynorphin's (+0.15) for the
  // *same* conflict words, so a bare-minimum conflict turn's cck deviation
  // often sits well under DEVIATION_FLOOR even though the same turn clears
  // it easily for sad. `floor: 0.12` compensates for that asymmetry — it is
  // not "angry is easier to trigger", it's "cck needs a lower bar to be
  // heard at all over the same-strength sad drives" — and the weight is
  // raised to 1.3 so that once heard, it isn't drowned out by cortisol +
  // dynorphin's combined score on a genuinely angry (not just sad) turn.
  { key: "adrenaline", direction: "up", emotion: "angry", weight: 1.1 },
  { key: "cck", direction: "up", emotion: "angry", weight: 1.3, floor: 0.12 },

  // relaxed: the inhibitory/settled/comfort drives, all triggered by calm
  // or night/comfort words.
  { key: "gaba", direction: "up", emotion: "relaxed", weight: 1.0 },
  { key: "glycine", direction: "up", emotion: "relaxed", weight: 0.8 },
  { key: "melatonin", direction: "up", emotion: "relaxed", weight: 0.6 },
  { key: "npy", direction: "up", emotion: "relaxed", weight: 0.6 },
  { key: "enkephalin", direction: "up", emotion: "relaxed", weight: 0.7 },

  // surprised: acetylcholine/orexin/histamine are still the same
  // length/question-shaped arousal drives used as MODIFIER_VOTES elsewhere
  // (see `char_count >= 16`, `question`, `novelty` in affect.rs), just
  // voting for a different expression here — that hasn't changed. What has
  // changed since SURPRISED_FLOOR was first set: affect.rs now also adds a
  // dedicated `b(surprise, 0.4/0.35/0.35)` delta to these same three drives
  // for actual surprise vocabulary (see `AffectState::update`'s SURPRISE
  // word list), so there is now a real content signal behind them, not just
  // sentence shape. Gating them like the other modifiers (only count once a
  // *surprised* primary already fired) would still make surprised
  // permanently unreachable, since nothing else ever originates that
  // primary — so they keep their own floor rather than the modifier
  // treatment. SURPRISED_FLOOR's *job* changed though: it no longer exists
  // to compensate for "no real signal exists" (there is one now), it exists
  // to stop the length/question-shaped component from firing surprised on
  // its own on an ordinary long reply — re-measured at 0.35 against the
  // current dataset (see the module doc comment's re-tuning round) and that
  // dropped dev strict accuracy from 49.7% to 43.4%, surprised's predicted
  // count ballooning from 33 to 67 against a support of only 25 as ordinary
  // long sentences tripped it again; 0.5 remains the right value, just for
  // the "stop sentence-shape false positives" reason above rather than the
  // original "no signal exists at all" one.
  { key: "acetylcholine", direction: "up", emotion: "surprised", weight: 0.6, floor: SURPRISED_FLOOR },
  { key: "orexin", direction: "up", emotion: "surprised", weight: 0.5, floor: SURPRISED_FLOOR },
  { key: "histamine", direction: "up", emotion: "surprised", weight: 0.4, floor: SURPRISED_FLOOR },
];

/**
 * Drives that move mainly with utterance *shape* (length, question marks)
 * rather than content — see the module doc comment's point 3. They cannot
 * originate a score: each only counts once the emotion it modifies already
 * has a primary vote on the board this frame (checked via `scores[emotion]
 * > 0` in `computeScores`), and even then at a fraction of a primary vote's
 * weight. That keeps "the sentence was long" from ever being sufficient by
 * itself, while still letting it sharpen an already-real signal (a long,
 * enthusiastic reply reads a little happier than a short one saying the
 * same thing).
 */
const MODIFIER_VOTES: readonly Vote[] = [
  { key: "dopamine", direction: "up", emotion: "happy", weight: 0.25 },
  { key: "dopamine", direction: "down", emotion: "sad", weight: 0.2 },
  { key: "noradrenaline", direction: "up", emotion: "angry", weight: 0.25 },
];

/**
 * Neutral's score when nothing else is happening — i.e. its ceiling. Picked
 * so that neutral wins outright at rest (all other scores 0) and still
 * beats a single mild primary vote sitting just past DEVIATION_FLOOR (a
 * lone vote there scores roughly `DEVIATION_FLOOR * weight`, well under
 * 0.3 for every weight in PRIMARY_VOTES), while a turn with one clear,
 * strong signal (deviation well past the floor, or several drives agreeing)
 * still outscores it once NEUTRAL_DECAY below is subtracted.
 */
const NEUTRAL_BASE = 0.3;

/**
 * How fast neutral's score gives up ground as the other five emotions'
 * combined score rises. At 0.5, a single primary vote at DEVIATION_FLOOR
 * (~0.2 * weight, so ~0.14-0.22 for most weights) trims neutral by well
 * under half of NEUTRAL_BASE — a lone borderline signal doesn't usually
 * flip the winner — while two agreeing primary votes, or one strong one,
 * comfortably push the challenger ahead.
 */
const NEUTRAL_DECAY = 0.5;

/**
 * Multiplier applied to every non-neutral score while the conversation is
 * closing (a farewell was just exchanged) or the partner is away (silent
 * past the absence timeout — see `PartnerAffect::check_absence`). Neither
 * condition is a reason to keep a strong expression on: a closing exchange
 * should settle rather than keep escalating, and there's nobody in front of
 * the NPC for an away partner's last emotional state to still be "for". This
 * nudges the decision toward neutral (via NEUTRAL_DECAY seeing a smaller
 * `otherTotal`) without hard-forcing it, so a turn that is both closing and
 * genuinely sad (see eval turn "somber-goodbye") can still come through.
 */
const WIND_DOWN_DAMPING = 0.5;

/**
 * cck's angry floor (see PRIMARY_VOTES) is deliberately low (0.12) to make
 * up for its small affect.rs delta relative to cortisol/dynorphin's for the
 * same conflict trigger. That low floor turns out to be too easy to clear
 * while `affect.inviteCaution` is active: affect.rs's LURE word list is
 * broad enough to match ordinary phrasing ("一緒に" in "一緒に感想でも話し
 * たい", "行こう" in "公園にでも行こうかな") that isn't actually a
 * suspicious invitation, and while caution is active it adds a flat
 * `cck += 0.2` on top of whatever the turn's actual content did (see
 * `inviteCautionBias`'s doc comment) — exactly the "arousal that isn't
 * valence" failure this file's design is otherwise trying to avoid. Rather
 * than lower cck's weight (which would also blunt it for the turns it
 * should catch), cck needs the normal, higher floor during invite caution;
 * `inviteCautionBias` already subtracts the *flat* per-turn contribution,
 * this covers the part of it that lingers across the ~3 turns caution stays
 * active, which the single-frame subtraction alone doesn't reach.
 */
const INVITE_CAUTION_CCK_FLOOR = 0.35;

function emptyScores(): Record<EmotionName, number> {
  return { neutral: 0, happy: 0, angry: 0, sad: 0, relaxed: 0, surprised: 0 };
}

/** This frame's score for all six expressions — always every key, 0 where
 *  nothing argues for that expression. `neutral` is computed last, from how
 *  much the other five collectively scored (see NEUTRAL_BASE/NEUTRAL_DECAY). */
function computeScores(affect: AffectSnapshot | null): Record<EmotionName, number> {
  const scores = emptyScores();
  if (!affect) {
    scores.neutral = NEUTRAL_BASE;
    return scores;
  }

  const driveByKey = new Map(affect.drives.map((d) => [d.key, d]));

  for (const vote of PRIMARY_VOTES) {
    const drive = driveByKey.get(vote.key);
    if (!drive) continue;
    const deviation = voteDeviation(drive, vote.key, vote.direction, affect);
    // See INVITE_CAUTION_CCK_FLOOR's doc comment: cck's floor is raised back
    // up while invite caution is active, since that's exactly the state its
    // deliberately-low default floor is too easy to clear in.
    const floor = vote.key === "cck" && affect.inviteCaution ? INVITE_CAUTION_CCK_FLOOR : (vote.floor ?? DEVIATION_FLOOR);
    if (deviation < floor) continue;
    scores[vote.emotion] += deviation * vote.weight;
  }

  for (const vote of MODIFIER_VOTES) {
    // Gate: a modifier can only sharpen a score a primary vote already
    // started (see MODIFIER_VOTES's doc comment) — never originate one.
    if (scores[vote.emotion] <= 0) continue;
    const drive = driveByKey.get(vote.key);
    if (!drive) continue;
    const deviation = voteDeviation(drive, vote.key, vote.direction, affect);
    if (deviation < DEVIATION_FLOOR) continue;
    scores[vote.emotion] += deviation * vote.weight;
  }

  if (affect.closing || affect.partnerAway) {
    for (const emotion of NON_NEUTRAL_EMOTIONS) scores[emotion] *= WIND_DOWN_DAMPING;
  }

  let otherTotal = 0;
  for (const emotion of NON_NEUTRAL_EMOTIONS) otherTotal += scores[emotion];
  scores.neutral = Math.max(0, NEUTRAL_BASE - NEUTRAL_DECAY * otherTotal);

  return scores;
}

/** Highest-scoring emotion, `EMOTIONS` order breaking ties (so neutral wins
 *  a tie over any other emotion, matching its role as the resting state). */
function argmaxEmotion(scores: Record<EmotionName, number>): EmotionName {
  let best = EMOTIONS[0];
  let bestScore = scores[best];
  for (const emotion of EMOTIONS) {
    if (scores[emotion] > bestScore) {
      best = emotion;
      bestScore = scores[emotion];
    }
  }
  return best;
}

/** 1 affect frame's worth of expression judgement. `previous` as the next
 *  frame's argument. */
export interface EmotionDecision {
  /** The expression to wear right now. */
  readonly emotion: EmotionName;
  /** How many frames in a row (including this one) `emotion` has been worn.
   *  1 the first frame it is picked. */
  readonly held: number;
  /** This frame's per-expression scores. Always has all six EmotionName
   *  keys, 0 where nothing argues for that expression — the eval report and
   *  any debug UI depend on that being a complete map, not a sparse one. */
  readonly scores: Readonly<Record<EmotionName, number>>;
}

/** Sentinel "nothing decided yet" value, for callers that want a non-null
 *  initial `previous` (e.g. a `useState` default) instead of threading
 *  `null` through their own state. `held: 0` is what marks it as not a real
 *  decision: `decideEmotion` treats `previous.held <= 0` exactly like
 *  `previous === null` (see its doc comment), so passing this in behaves
 *  identically to passing `null` on the first real frame. */
export const NEUTRAL_DECISION: EmotionDecision = {
  emotion: "neutral",
  held: 0,
  scores: emptyScores(),
};

/**
 * How much a challenger's score must exceed the currently-worn emotion's
 * score by before it is allowed to take over. Without this, two expressions
 * scoring within a hair of each other (common near DEVIATION_FLOOR — see
 * the affect-dynamics diagnosis's §3b/§5, where the true per-frame winner
 * flipped turn to turn) would make the face flicker between them every
 * frame the scores happen to cross. 0.13 is still a little under one mild
 * primary vote's typical contribution (~0.2 deviation * a 0.6-1.1 weight),
 * so a challenger needs roughly one clear additional signal over what's
 * already backing the current expression, not just noise, to dislodge it.
 *
 * Lowered from 0.15 in the re-tuning round documented in the module doc
 * comment: predicted switches were 49 against a gold count of 97 — the face
 * was changing about half as often as it should — while flickers sat at 8
 * against a gold of 23, well clear of that ceiling. That gap between "way
 * under target" and "way under the flicker ceiling" is exactly the case for
 * loosening the margin. Measured in 0.01 steps against dev: 0.13 is where
 * dev strict accuracy peaks (49.7%, versus 48.7% at 0.15) while the
 * dev-holdout gap stays small (+2.3pp); 0.12 gave the same switch count for
 * slightly lower dev accuracy, and 0.10-0.11 started trading dev accuracy
 * away for switches that mostly weren't matching gold's timing anyway (and
 * pushed `neutral` recall under the 60% floor). Predicted switches landed
 * at 53 at this setting — still under gold's 97, but flickers only rose to
 * 7, nowhere near the 23 ceiling, so there was no flicker cost to this move
 * at all, only an accuracy trade to stop at the right point.
 */
const SWITCH_MARGIN = 0.13;

/**
 * Minimum number of frames (== conversation turns, since decideEmotion runs
 * once per affect frame and affect frames arrive one per partner utterance)
 * the *currently worn* expression must already have been held before a
 * challenger clearing SWITCH_MARGIN is allowed to replace it — protection
 * against a two-frame flash-and-revert on top of what the margin alone
 * gives.
 *
 * Set to 1 (i.e. no additional protection beyond the margin) rather than
 * something like 2, on measured evidence rather than by default: raising it
 * to 2 consistently cost several points of strict accuracy against
 * eval/emotion/trace.json (turns emphatically switching expression turn
 * over turn, e.g. a sharp "that annoyed me" reply right after a neutral
 * greeting, were held at the stale expression for one extra turn) for a
 * flicker count that was already low (SWITCH_MARGIN is doing most of the
 * real stabilizing work — see its own doc comment) — not a trade worth
 * making by default. The constant, and `EmotionDecision.held` that feeds
 * it, are kept rather than removed because the trade-off direction can
 * easily flip once the affect.rs saturation fix lands (a noisier per-frame
 * signal would make the extra protection worth more), at which point
 * raising this one number is the whole fix.
 */
const MIN_HOLD_FRAMES = 1;

/**
 * 1 affect frame -> the expression the NPC should be wearing, given both
 * this frame's scores and (if any) the expression it was already wearing.
 *
 * Pure function: no module-scoped mutable state. Passing the previous
 * return value back in as `previous` is what carries the hysteresis
 * (SWITCH_MARGIN, MIN_HOLD_FRAMES) forward; `null` (or `NEUTRAL_DECISION`,
 * or any decision with `held <= 0`) means "nothing decided yet", so the
 * frame's argmax is taken outright with no hysteresis to satisfy.
 */
export function decideEmotion(affect: AffectSnapshot | null, previous: EmotionDecision | null): EmotionDecision {
  const scores = computeScores(affect);

  if (previous === null || previous.held <= 0) {
    return { emotion: argmaxEmotion(scores), held: 1, scores };
  }

  const current = previous.emotion;
  const challenger = argmaxEmotion(scores);
  if (challenger === current) {
    return { emotion: current, held: previous.held + 1, scores };
  }

  const clearsMargin = scores[challenger] > scores[current] + SWITCH_MARGIN;
  const heldLongEnough = previous.held >= MIN_HOLD_FRAMES;
  if (clearsMargin && heldLongEnough) {
    return { emotion: challenger, held: 1, scores };
  }
  return { emotion: current, held: previous.held + 1, scores };
}
