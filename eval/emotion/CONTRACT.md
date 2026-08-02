# Expression-evaluation contract

The face the NPC wears is produced by two components that live in different
languages:

1. `crates/npc-talk/src/affect.rs` — the 22-drive affect model, updated once
   per partner utterance, published to the UI as an `affect` frame.
2. `web/src/lib/vrm-emotion.ts` — `emotionFromAffect`, which reduces one
   affect frame to one of six VRM expressions.

Neither can be evaluated alone: the drive model has no notion of a face, and
the mapping has no notion of a conversation. So the eval loop runs the real
Rust model over a scripted conversation, dumps its frames, and scores the
real TS mapping against hand-labelled expectations.

    dataset.jsonl  --(cargo run --example affect_trace)-->  trace.json
    trace.json + dataset.jsonl  --(vitest emotion-eval)-->  report

Three artifacts, three owners, three file-disjoint pieces of work. The JSON
shapes below are the seam between them and must not be changed unilaterally.

## 1. `eval/emotion/dataset.jsonl`

JSON Lines. One object per line = one whole conversation, replayed in order
against a single `PartnerAffect`.

```json
{
  "id": "kebab-case-unique-id",
  "split": "dev",
  "note": "what this conversation is probing, one line",
  "turns": [
    {
      "speaker": "太郎",
      "text": "こんにちは、はじめまして",
      "reply": "……こんにちは。",
      "expect": "neutral",
      "alsoOk": ["relaxed"],
      "why": "初対面の挨拶。まだ何も動いていない"
    }
  ]
}
```

Conversation fields:

| field   | required | meaning |
| ------- | -------- | ------- |
| `id`    | yes      | kebab-case, unique across the file. |
| `split` | yes      | `"dev"` or `"holdout"`. See "dev / holdout split" below. |
| `note`  | no       | what this conversation is probing, one line. |
| `turns` | yes      | the scripted exchange, replayed in order against a single `PartnerAffect`. |

### dev / holdout split

Every conversation carries `split: "dev" | "holdout"`. Holdout is about
**1/3 of conversations**, chosen by stratified sampling: every scenario
category this dataset covers (first meeting, joy, humor, conflict, sadness,
surprise, calm, farewell, speaker handoff, long ordinary conversation,
mid-conversation emotion switch) has at least one conversation in dev *and*
at least one in holdout. A category that landed entirely in one split would
turn that split into "a different difficulty," not an overfitting probe —
stratifying is what keeps the two splits comparable.

The reason this exists: `affect.rs`'s keyword vocabulary and
`vrm-emotion.ts`'s thresholds get tuned by developers who can see this
dataset's `text` strings. It is easy — even unintentionally — to tune a
vocabulary list until it fits the exact phrasings dev turns happen to use,
without the model actually generalizing to phrasings it hasn't seen. Holdout
exists to catch that: it is real conversation data, scored the same way, but
nobody should be reading its `text` values while deciding what word to add
to a `const` list or what threshold to nudge. If strict accuracy on dev
climbs while holdout stays flat (or drops), that gap **is** the overfitting
signal — see `emotion-eval.ts`'s per-split reporting.

Turn fields:

| field     | required | meaning |
| --------- | -------- | ------- |
| `speaker` | no       | display name of who spoke; omit for the unnamed web-UI partner. Drives `PartnerAffect::observe`. |
| `text`    | yes      | the partner's utterance — the ONLY thing the current model reads. |
| `reply`   | no       | what the NPC said back. **Not consumed today**; recorded so a later change that folds the NPC's own reply into affect can be evaluated against the same labels. |
| `expect`  | yes      | the gold expression: one of `neutral` `happy` `angry` `sad` `relaxed` `surprised`. |
| `alsoOk`  | no       | other expressions a reasonable viewer would accept for this turn. Scored as a separate "lenient" accuracy; `expect` alone is "strict". |
| `why`     | yes      | one line justifying the label, in Japanese. This is the label's provenance — a label nobody can justify is a bad label. |

## 2. `eval/emotion/trace.json` (generated — gitignored)

Written by `cargo run -p npc-talk --example affect_trace`.

```json
{
  "conversations": [
    {
      "id": "kebab-case-unique-id",
      "turns": [
        {
          "index": 0,
          "speaker": "太郎",
          "text": "こんにちは、はじめまして",
          "affect": {
            "drives": [{ "key": "dopamine", "level": 0.5, "base": 0.5 }],
            "familiarity": 0.1,
            "closing": false,
            "inviteCaution": false,
            "partner": "太郎",
            "partnerKnown": false,
            "partnerSwitched": true,
            "partnerAway": false
          }
        }
      ]
    }
  ]
}
```

`affect` is `npc_talk::affect::AffectSnapshot` serialized verbatim — the same
camelCase shape the web UI already receives over the websocket. It carries no
`ts`; the TS side supplies `ts: 0` when widening it to `AffectSnapshot`.

`index` is the 0-based turn index within the conversation, and lines up
positionally with `dataset.jsonl`'s `turns` array for that `id`.

## 3. `web/src/lib/emotion-eval.ts`

Pure scoring over the two files above, plus
`web/src/lib/emotion-eval.test.ts` as the runner. Reports strict accuracy,
lenient accuracy, a gold×predicted confusion matrix, and the per-turn
mismatches, so a change to either component can be judged rather than
guessed at.

Every report is broken down three ways — overall, dev-only, holdout-only —
with the dev/holdout strict-accuracy gap called out explicitly, since that
gap is the thing this whole `split` mechanism exists to expose. Each
breakdown is also printed alongside the strict accuracy of the trivial
"always predict neutral" constant model, since that is the real floor a
tuned model needs to clear.
