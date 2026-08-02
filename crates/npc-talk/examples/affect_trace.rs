//! Replays the labelled conversations in `eval/emotion/dataset.jsonl`
//! through the real affect model and dumps the `affect` frame it would have
//! published after every partner utterance, so `web/src/lib/emotion-eval.ts`
//! can score the VRM-expression mapping against hand-labelled expectations
//! without a TypeScript reimplementation of `npc_talk::affect` to keep in
//! sync. See `eval/emotion/CONTRACT.md` for the exact shape of both the
//! input and the output written here.
//!
//! Mirrors `ChatEngine::run_turn`'s affect sequence (`npc-talk/src/engine.rs`,
//! around line 300) turn for turn — `check_absence` -> `observe` -> `update`
//! -> snapshot, in that order. Diverging from it would let the trace drift
//! from the face the NPC actually wears in production.
//!
//! Usage: `cargo run -p npc-talk --example affect_trace [dataset] [output]`,
//! both paths defaulting relative to the repo root (see [`DEFAULT_DATASET`]
//! / [`DEFAULT_OUTPUT`]).

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Context;
use npc_talk::affect::{AffectSnapshot, PartnerAffect};
use serde::{Deserialize, Serialize};

const DEFAULT_DATASET: &str = "eval/emotion/dataset.jsonl";
const DEFAULT_OUTPUT: &str = "eval/emotion/trace.json";

/// `talk.affect.absence_timeout_secs`'s default, copied from
/// `config.example.json` / `npc_core::config::default_absence_timeout_secs`
/// (that function is private to `npc-core`, so it can't be imported here —
/// keep this in sync by hand if the shipped default ever changes).
const ABSENCE_TIMEOUT_SECS: u64 = 300;

/// How far `now` advances between turns within one conversation. Far below
/// [`ABSENCE_TIMEOUT_SECS`] so a normal scripted exchange never trips
/// `check_absence` by accident — every conversation in the dataset is meant
/// to test the drive model, not the absence timer.
const TURN_STEP_SECS: u64 = 10;

/// One line of `dataset.jsonl` — a whole conversation replayed against a
/// single `PartnerAffect`. Only the fields the model actually reads are
/// declared; CONTRACT.md's `note`, and each turn's `reply` / `expect` /
/// `alsoOk` / `why`, are eval-scoring metadata for
/// `web/src/lib/emotion-eval.ts` and are deliberately left undeclared here
/// so their presence in the dataset is ignored rather than rejected (no
/// `deny_unknown_fields`).
#[derive(Debug, Deserialize)]
struct DatasetConversation {
    id: String,
    turns: Vec<DatasetTurn>,
}

#[derive(Debug, Deserialize)]
struct DatasetTurn {
    /// Display name of who spoke, or absent for the unnamed web-UI partner
    /// — drives `PartnerAffect::observe`. `#[serde(default)]` because a
    /// missing key, not just an explicit `null`, has to read as `None`.
    #[serde(default)]
    speaker: Option<String>,
    /// The only thing the current model reads.
    text: String,
}

/// `eval/emotion/trace.json`'s top level (CONTRACT.md section 2).
#[derive(Debug, Serialize)]
struct Trace {
    conversations: Vec<ConversationTrace>,
}

#[derive(Debug, Serialize)]
struct ConversationTrace {
    id: String,
    turns: Vec<TurnTrace>,
}

#[derive(Debug, Serialize)]
struct TurnTrace {
    /// 0-based index within the conversation, lining up positionally with
    /// `dataset.jsonl`'s `turns` array for the same `id`.
    index: usize,
    speaker: Option<String>,
    text: String,
    /// Serialized verbatim — already `#[serde(rename_all = "camelCase")]`
    /// on the Rust side, matching the wire shape the web UI receives.
    affect: AffectSnapshot,
}

fn main() -> anyhow::Result<()> {
    let mut args = env::args().skip(1);
    let dataset_path = args.next().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(DEFAULT_DATASET));
    let output_path = args.next().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(DEFAULT_OUTPUT));

    let raw = fs::read_to_string(&dataset_path)
        .with_context(|| format!("reading dataset {}", dataset_path.display()))?;

    let conversations: Vec<DatasetConversation> = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<DatasetConversation>(line)
                .with_context(|| format!("parsing dataset line: {line}"))
        })
        .collect::<anyhow::Result<_>>()?;

    let mut turn_count = 0usize;
    let mut traced = Vec::with_capacity(conversations.len());

    for conversation in &conversations {
        // A fresh `PartnerAffect` per conversation: state never carries over
        // between the dataset's entries, exactly as a brand-new NPC session
        // would start cold for each one.
        let mut affect = PartnerAffect::default();
        let start = Instant::now();
        let mut turns = Vec::with_capacity(conversation.turns.len());

        for (index, turn) in conversation.turns.iter().enumerate() {
            let now = start + Duration::from_secs(TURN_STEP_SECS * index as u64);

            // Same order `ChatEngine::run_turn` uses: catch up on absence,
            // then attribute the turn to its speaker, then fold the
            // utterance in, then snapshot.
            affect.check_absence(now, Duration::from_secs(ABSENCE_TIMEOUT_SECS));
            affect.observe(turn.speaker.as_deref());
            affect.update(&turn.text, now);

            turns.push(TurnTrace {
                index,
                speaker: turn.speaker.clone(),
                text: turn.text.clone(),
                affect: affect.snapshot(),
            });
        }

        turn_count += turns.len();
        traced.push(ConversationTrace { id: conversation.id.clone(), turns });
    }

    let conversation_count = traced.len();
    let trace = Trace { conversations: traced };
    let json = serde_json::to_string_pretty(&trace)?;

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    fs::write(&output_path, json).with_context(|| format!("writing trace {}", output_path.display()))?;

    println!("{conversation_count} conversations, {turn_count} turns -> {}", output_path.display());

    Ok(())
}
