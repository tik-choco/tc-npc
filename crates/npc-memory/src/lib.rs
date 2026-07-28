//! Memory module (ports Go `agent-memory`). Maintains a rolling short-term
//! summary of recent chat turns and a long-term vector store, driven by the
//! in-process bus instead of Redis pub/sub.
//!
//! - Every `agent:chat` / `chat_log` turn this module actually receives is
//!   appended verbatim to `{data_dir}/chat-log.jsonl` via
//!   `npc_core::chatlog::append`, regardless of whether long-term memory
//!   (`api.embedding_model`) is configured — so raw conversation history
//!   survives independently of summarization/RAG being on or off. This
//!   guarantee has two edges, though: it only holds while this module is
//!   running at all (it's spawned only when `config.memory.enabled` is
//!   true, see `src/main.rs`), and only for turns that were published in
//!   the first place — `npc-talk` publishes `chat_log` after a successful
//!   LLM reply, so a turn where the chat completion call itself errored out
//!   is never published and never reaches the log, even though the user's
//!   message already appeared in the UI.
//! - `agent:chat` / `chat_log` turns accumulate in a short-term buffer.
//! - Every ~5 turns, or after `memory.idle_timeout_minutes` of inactivity,
//!   the buffer is summarized (LLM) into a rolling short-term summary,
//!   published as `short_term_memory` on `agent:mem`.
//! - On idle timeout, the buffer is additionally consolidated (LLM) into
//!   long-term memory: chunked, embedded, and stored in a JSON vector
//!   store, then the buffer is cleared.
//! - `agent:sense` / `speech` or `vision` messages trigger a RAG search
//!   over the long-term store; hits are published as `long_term_memory` on
//!   `agent:mem`.
//!
//! If `api.embedding_model` is empty, all embedding/RAG behavior is
//! skipped (logged once) but short-term summarization keeps working.
//!
//! ## Person memory (`config.memory.people`, see [`person::PersonTracker`])
//!
//! When `config.memory.people.enabled` is true, this module also keeps a
//! [`person::PersonTracker`] roster alongside the short/long-term state
//! above — entirely additively; none of the behavior described above
//! changes when this is on, and all of it is skipped when it's off:
//!
//! - A `speech` sense payload with a `speaker` field resolves that name to
//!   a person (creating one if needed) and remembers it as the "current
//!   speaker". The first time the speaker changes, that person's profile is
//!   rendered ([`person::PersonTracker::profile_text`]) and published as
//!   `person_memory` on `agent:mem`, which npc-talk injects as
//!   `{{person_memory}}`.
//! - While a speaker is known, the RAG search triggered by `speech`/`vision`
//!   uses [`store::VectorStore::search_for_person`] instead of plain
//!   `search`, so that speaker's own long-term memories are favored (without
//!   excluding general knowledge).
//! - Long-term consolidation tags the stored chunk's metadata with the
//!   current speaker's `person_id` (when known), so a later
//!   `search_for_person` for that person can find it again.
//! - On the same idle-timeout tick that triggers consolidation, the
//!   conversation buffer is also run through `config.memory.people.extract_prompt`
//!   to pull out per-person facts (a JSON array), which are merged into each
//!   matched person's `facts` (capped at `max_facts`, see
//!   [`person::PersonTracker::add_facts`]).
//! - If `config.memory.people.link_vision` is true, `agent:sense` /
//!   `person_seen` payloads (from npc-vision) are merged into the roster via
//!   [`person::PersonTracker::observe_from_vision`].
//!
//! Every person mutation is persisted (`npc_core::save_person`) and
//! published as `person_updated` on `npc:ui` so the web UI and npc-server
//! stay in sync.

mod person;
mod store;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio::time::Instant;

use npc_core::bus::{msg, topic};
use npc_core::{Bus, Module, ModuleCtx};
use npc_llm::{ChatMessage, ChatRequest, LlmClient};

use person::PersonTracker;
use store::VectorStore;

const STORE_FILE_NAME: &str = "memory-store.json";
/// Short-term buffer flushes to a summary after this many turns, matching
/// the Go original's `config.MemorySummaryThreshold`.
const SUMMARY_TURN_THRESHOLD: usize = 5;

pub fn module(_ctx: &ModuleCtx) -> anyhow::Result<Box<dyn Module>> {
    Ok(Box::new(MemoryModule))
}

struct MemoryModule;

#[async_trait]
impl Module for MemoryModule {
    fn name(&self) -> &'static str {
        "npc-memory"
    }

    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()> {
        let cfg = ctx.config.clone();

        // Resolved once at startup (no hot-reload for either connection,
        // matching the pre-preset behavior). Chat (summarize/consolidate)
        // and embedding can now point at different providers, so they're
        // resolved independently; a second `LlmClient` is only built when
        // they actually differ, so the common case (same provider, as with
        // every pre-preset config) still shares one client/connection pool.
        let resolved_chat = cfg.resolve_llm(npc_core::LlmTask::Memory);
        let resolved_embedding = cfg.resolve_llm(npc_core::LlmTask::Embedding);
        let llm = Arc::new(
            LlmClient::new(resolved_chat.base_url.clone(), resolved_chat.api_key.clone())
                .with_reasoning_effort(resolved_chat.reasoning_effort.clone()),
        );
        let embed_llm = if resolved_embedding.base_url == resolved_chat.base_url
            && resolved_embedding.api_key == resolved_chat.api_key
        {
            llm.clone()
        } else {
            Arc::new(LlmClient::new(resolved_embedding.base_url.clone(), resolved_embedding.api_key.clone()))
        };
        let chat_model = resolved_chat.model;
        let embedding_model = resolved_embedding.model;

        // Summaries are fed back to npc-talk as `{{short_term_memory}}` /
        // `{{long_term_memory}}`, so they follow `config.language` too — a
        // Japanese-worded summarize prompt would otherwise pull the whole
        // memory context back into Japanese.
        let summarize_prompt =
            npc_core::config::with_language_instruction(&cfg.memory.summarize_prompt, &cfg.language);
        let consolidate_prompt = npc_core::config::with_language_instruction(
            &cfg.memory.consolidate_prompt,
            &cfg.language,
        );

        let store_path = ctx.data_dir.join(STORE_FILE_NAME);
        let store = Arc::new(Mutex::new(VectorStore::load(&store_path)));

        // Person memory: entirely off (no roster load, no bus handling) when
        // `config.memory.people.enabled` is false, so an old config keeps
        // behaving exactly as before.
        let people_enabled = cfg.memory.people.enabled;
        let link_vision = cfg.memory.people.link_vision;
        let person_extract_prompt = cfg.memory.people.extract_prompt.clone();
        let mut tracker = if people_enabled {
            Some(PersonTracker::load(&ctx.data_dir, cfg.memory.people.max_facts))
        } else {
            None
        };
        // The person currently believed to be speaking, tracked purely from
        // `speech` sense payloads' `speaker` field. Used to bias RAG search
        // and to tag newly consolidated long-term chunks.
        let mut current_speaker: Option<String> = None;

        if embedding_model.is_empty() {
            tracing::info!(
                "npc-memory: api.embedding_model is empty; RAG search and long-term embedding are disabled, short-term summaries still run"
            );
        }

        let mut bus_rx = ctx.bus.subscribe();
        let idle_duration = Duration::from_secs(60 * cfg.memory.idle_timeout_minutes.max(1) as u64);
        let sleep = tokio::time::sleep(idle_duration);
        tokio::pin!(sleep);

        // Short-term state, owned by the loop (single-threaded mutation,
        // matching the Go original's non-concurrent `currentLogs`/`lastSummary`).
        let mut buffer: Vec<String> = Vec::new();
        let mut summary = String::new();

        loop {
            tokio::select! {
                _ = ctx.shutdown.cancelled() => {
                    tracing::info!("npc-memory: shutdown requested, stopping");
                    // `tokio::select!` without `biased` picks a ready branch
                    // pseudo-randomly when more than one is ready. If npc-talk
                    // just published `chat_log` for the reply the user is
                    // reading right as they close the app, this shutdown
                    // branch and the `bus_rx.recv()` branch below can both be
                    // ready at once — and if shutdown wins, that `chat_log`
                    // is never read off the bus, so it never reaches
                    // `chat-log.jsonl`. Drain whatever is already queued on
                    // the bus (non-blocking) before doing anything else, so
                    // that last turn is still persisted.
                    drain_pending_chat_logs(&mut bus_rx, &ctx.data_dir, &mut buffer);

                    // Flush whatever the buffer still holds (turns that hadn't hit
                    // the idle timeout yet) into long-term memory before exiting,
                    // reusing the same `consolidate_long_term` call the idle-timeout
                    // path uses. When `embedding_model` is empty this is skipped —
                    // that's not a data-loss risk, since every raw turn was already
                    // durably appended to `chat-log.jsonl` as it arrived (see the
                    // `chat_log` handling above).
                    if !buffer.is_empty() && !embedding_model.is_empty() {
                        let turns = std::mem::take(&mut buffer);
                        let prev_summary = summary.clone();
                        let consolidate = consolidate_long_term(
                            &llm,
                            &embed_llm,
                            &chat_model,
                            &embedding_model,
                            &consolidate_prompt,
                            &prev_summary,
                            &turns,
                            cfg.memory.chunk_size as usize,
                            cfg.memory.chunk_overlap as usize,
                            &store,
                            &ctx,
                            current_speaker.as_deref(),
                        );
                        // Shutdown should stay fast; give the LLM/embedding calls a
                        // bounded window and give up (logging) rather than hang.
                        if tokio::time::timeout(Duration::from_secs(10), consolidate).await.is_err() {
                            tracing::warn!("npc-memory: shutdown long-term consolidation timed out, giving up");
                        }
                    }
                    return Ok(());
                }
                _ = &mut sleep => {
                    if !buffer.is_empty() {
                        tracing::debug!("npc-memory: idle timeout reached, summarizing and consolidating");
                        let turns = std::mem::take(&mut buffer);
                        let prev_summary = summary.clone();

                        let new_summary = run_summarize(&llm, &chat_model, &summarize_prompt, &prev_summary, &turns).await;
                        if let Some(new_summary) = new_summary {
                            summary = new_summary.clone();
                            ctx.bus.publish(topic::MEM, msg::SHORT_TERM_MEMORY, json!({ "content": new_summary }));
                        }

                        if !embedding_model.is_empty() {
                            consolidate_long_term(
                                &llm,
                                &embed_llm,
                                &chat_model,
                                &embedding_model,
                                &consolidate_prompt,
                                &prev_summary,
                                &turns,
                                cfg.memory.chunk_size as usize,
                                cfg.memory.chunk_overlap as usize,
                                &store,
                                &ctx,
                                current_speaker.as_deref(),
                            )
                            .await;
                        }

                        // Same idle tick as consolidation: pull per-person
                        // facts out of the just-flushed buffer.
                        if let Some(tracker) = tracker.as_mut() {
                            extract_person_facts(
                                &llm,
                                &chat_model,
                                &person_extract_prompt,
                                &turns,
                                tracker,
                                &ctx.bus,
                            )
                            .await;
                        }
                    }
                    sleep.as_mut().reset(Instant::now() + idle_duration);
                }
                incoming = bus_rx.recv() => {
                    match incoming {
                        Ok(bus_msg) => {
                            match (bus_msg.topic.as_str(), bus_msg.env.r#type.as_str()) {
                                (topic::CHAT, msg::CHAT_LOG) => {
                                    // Persist the raw turn immediately, independent of
                                    // `embedding_model` — the raw log is the durable
                                    // fallback even when summarization and long-term
                                    // embedding are disabled or fail. Note this module
                                    // is only spawned when `config.memory.enabled` is
                                    // true (see `src/main.rs`), so disabling memory
                                    // entirely still means no transcript on disk.
                                    if let Some(entry) = build_chat_log_entry(&bus_msg.env.payload) {
                                        if let Err(err) = npc_core::chatlog::append(&ctx.data_dir, &entry) {
                                            tracing::warn!(error = %err, "npc-memory: failed to append raw chat log");
                                        }
                                    }

                                    if let Some(turn) = parse_chat_log(&bus_msg.env.payload) {
                                        buffer.push(turn);
                                        sleep.as_mut().reset(Instant::now() + idle_duration);

                                        if buffer.len() >= SUMMARY_TURN_THRESHOLD {
                                            let turns = std::mem::take(&mut buffer);
                                            let prev_summary = summary.clone();
                                            if let Some(new_summary) = run_summarize(&llm, &chat_model, &summarize_prompt, &prev_summary, &turns).await {
                                                summary = new_summary.clone();
                                                ctx.bus.publish(topic::MEM, msg::SHORT_TERM_MEMORY, json!({ "content": new_summary }));
                                            }
                                        }
                                    }
                                }
                                (topic::SENSE, t) if t == msg::SPEECH || t == msg::VISION => {
                                    // Person tracking rides along on `speech` payloads
                                    // regardless of whether embedding/RAG is configured
                                    // (it needs no embedding model of its own).
                                    if people_enabled && t == msg::SPEECH {
                                        if let Some(speaker_name) = parse_speaker(&bus_msg.env.payload) {
                                            if let Some(tracker) = tracker.as_mut() {
                                                if let Some(person_id) = tracker
                                                    .observe_from_chat(&speaker_name)
                                                    .map(|p| p.id.clone())
                                                {
                                                    tracker.persist_and_publish(&ctx.bus, &person_id);

                                                    // Only (re)publish the profile when the
                                                    // speaker actually changed, so a long
                                                    // monologue from the same person doesn't
                                                    // spam `person_memory` on every line.
                                                    if current_speaker.as_deref() != Some(person_id.as_str()) {
                                                        if let Some(person) = tracker.get(&person_id) {
                                                            let content = tracker.profile_text(person);
                                                            ctx.bus.publish(
                                                                topic::MEM,
                                                                msg::PERSON_MEMORY,
                                                                json!({
                                                                    "person_id": person_id,
                                                                    "name": person.name,
                                                                    "content": content,
                                                                }),
                                                            );
                                                        }
                                                        current_speaker = Some(person_id);
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    if embedding_model.is_empty() {
                                        continue;
                                    }
                                    if let Some(content) = parse_sense_content(&bus_msg.env.payload) {
                                        tokio::spawn(handle_sense(
                                            content,
                                            embed_llm.clone(),
                                            embedding_model.clone(),
                                            store.clone(),
                                            ctx.bus.clone(),
                                            cfg.memory.top_k as usize,
                                            cfg.memory.threshold,
                                            current_speaker.clone(),
                                        ));
                                    }
                                }
                                (topic::SENSE, t) if t == msg::PERSON_SEEN => {
                                    if people_enabled && link_vision {
                                        if let Some(tracker) = tracker.as_mut() {
                                            if let Some(seen) = parse_person_seen(&bus_msg.env.payload) {
                                                if let Some(person_id) = tracker
                                                    .observe_from_vision(&seen.name, &seen.appearance, &seen.note)
                                                    .map(|p| p.id.clone())
                                                {
                                                    tracker.persist_and_publish(&ctx.bus, &person_id);
                                                }
                                            }
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "npc-memory: bus receiver lagged, some messages were dropped");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::warn!("npc-memory: bus channel closed, stopping");
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

/// Drain whatever `chat_log` messages are already sitting in the bus's
/// broadcast queue, non-blocking, and persist them the same way the normal
/// `bus_rx.recv()` branch does: append to `chat-log.jsonl` via
/// [`build_chat_log_entry`] + `npc_core::chatlog::append`, and push the
/// formatted turn (via [`parse_chat_log`]) onto `buffer` so a subsequent
/// consolidation flush still covers it.
///
/// Only called from the shutdown branch (see the race described there).
/// Deliberately narrow in scope, to keep shutdown fast:
/// - Only `CHAT_LOG` is handled here. `SENSE` messages (which would trigger
///   an embedding call + RAG search) and person-tracking are skipped — they
///   don't cause data loss the way a dropped `chat_log` does, and running
///   them would mean awaiting network calls during shutdown.
/// - This duplicates a few lines of the `bus_rx.recv()` match arm above
///   rather than sharing a helper with it, because that arm also drives the
///   `SUMMARY_TURN_THRESHOLD` check and `run_summarize(...).await`, which
///   doesn't belong in a non-blocking drain; factoring out just the
///   persistence step seemed like more indirection than the ~10 duplicated
///   lines are worth.
///
/// Bounded by construction: `try_recv` returns `Empty` once the queue that
/// existed when shutdown fired has been drained, which ends the loop.
fn drain_pending_chat_logs(
    bus_rx: &mut tokio::sync::broadcast::Receiver<npc_core::bus::BusMessage>,
    data_dir: &std::path::Path,
    buffer: &mut Vec<String>,
) {
    loop {
        match bus_rx.try_recv() {
            Ok(bus_msg) => {
                if bus_msg.topic == topic::CHAT && bus_msg.env.r#type == msg::CHAT_LOG {
                    if let Some(entry) = build_chat_log_entry(&bus_msg.env.payload) {
                        if let Err(err) = npc_core::chatlog::append(data_dir, &entry) {
                            tracing::warn!(error = %err, "npc-memory: failed to append raw chat log during shutdown drain");
                        }
                    }
                    if let Some(turn) = parse_chat_log(&bus_msg.env.payload) {
                        buffer.push(turn);
                    }
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                // Some messages were dropped, but the channel is still open
                // and may still hold more of what we're looking for — keep
                // draining rather than treating this like `Empty`/`Closed`.
                tracing::warn!(skipped, "npc-memory: bus receiver lagged during shutdown drain, some messages were dropped");
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
            | Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
        }
    }
}

// ---------------------------------------------------------------------
// payload parsing
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ChatLogPayload {
    #[serde(default)]
    input: String,
    #[serde(default)]
    output: String,
}

/// Parse a `chat_log` payload (`{input, output, ...}`) into a formatted
/// turn string (`User: ...\nAI: ...`), matching the Go original's
/// `config.LogFormat`. Returns `None` if both fields are missing/empty.
fn parse_chat_log(payload: &Value) -> Option<String> {
    let parsed: ChatLogPayload = serde_json::from_value(payload.clone()).ok()?;
    if parsed.input.is_empty() && parsed.output.is_empty() {
        return None;
    }
    Some(format!("User: {}\nAI: {}", parsed.input, parsed.output))
}

/// The full `chat_log` payload (`{input, output, time, speaker?}`, see
/// `npc_talk::engine::chat_log_payload`), used to build a durable
/// [`npc_core::chatlog::ChatLogEntry`] — kept separate from [`parse_chat_log`],
/// which only extracts the short-term-summary-formatted turn string.
#[derive(Debug, Deserialize)]
struct RawChatLogPayload {
    #[serde(default)]
    input: String,
    #[serde(default)]
    output: String,
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    speaker: Option<String>,
}

/// Build a [`npc_core::chatlog::ChatLogEntry`] from a `chat_log` payload.
/// `time` falls back to "now" (RFC3339) if the payload didn't carry one;
/// `speaker` is `None` when absent/blank. Returns `None` when both `input`
/// and `output` are empty, matching [`parse_chat_log`]'s behavior.
fn build_chat_log_entry(payload: &Value) -> Option<npc_core::chatlog::ChatLogEntry> {
    let parsed: RawChatLogPayload = serde_json::from_value(payload.clone()).ok()?;
    if parsed.input.is_empty() && parsed.output.is_empty() {
        return None;
    }
    let time = parsed
        .time
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    Some(npc_core::chatlog::ChatLogEntry {
        time,
        speaker: parsed.speaker.filter(|s| !s.is_empty()),
        input: parsed.input,
        output: parsed.output,
    })
}

/// Parse a `speech`/`vision` sense payload, which is either a bare string
/// or `{"content": "..."}`.
fn parse_sense_content(payload: &Value) -> Option<String> {
    let content = if let Some(s) = payload.as_str() {
        s.to_string()
    } else {
        payload.get("content")?.as_str()?.to_string()
    };
    if content.is_empty() {
        None
    } else {
        Some(content)
    }
}

/// The optional `speaker` field on a `speech` sense payload (person-memory
/// contract §2). `None` if absent, non-string, or blank.
fn parse_speaker(payload: &Value) -> Option<String> {
    let name = payload.get("speaker")?.as_str()?.trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// A `person_seen` payload from npc-vision (person-memory contract §2):
/// `{"name", "appearance", "note", "source"}`, all optional/defaultable —
/// `name` in particular is routinely empty (an unidentified sighting).
#[derive(Debug, Deserialize)]
struct PersonSeenPayload {
    #[serde(default)]
    name: String,
    #[serde(default)]
    appearance: String,
    #[serde(default)]
    note: String,
}

fn parse_person_seen(payload: &Value) -> Option<PersonSeenPayload> {
    serde_json::from_value(payload.clone()).ok()
}

/// One element of the person-fact-extraction LLM response:
/// `{"name": "...", "facts": ["...", ...]}`.
#[derive(Debug, Deserialize)]
struct ExtractedPersonFacts {
    #[serde(default)]
    name: String,
    #[serde(default)]
    facts: Vec<String>,
}

/// Pull a JSON array out of an LLM response that isn't guaranteed to be bare
/// JSON — local models routinely wrap it in a ```json fence, or add a
/// sentence of preamble/trailing commentary. Finds the first `[` and its
/// matching `]` (bracket-depth aware, skipping brackets inside string
/// literals) and parses just that slice, ignoring everything outside it.
/// Returns `None` if no balanced `[...]` span exists or it doesn't parse
/// into the expected shape.
fn parse_person_facts_response(response: &str) -> Option<Vec<ExtractedPersonFacts>> {
    let start = response.find('[')?;
    let bytes = response.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let mut end = None;

    // Scanning raw bytes (not `char`s) is safe here: every byte we compare
    // against ('"', '\\', '[', ']') is ASCII, and UTF-8 continuation/lead
    // bytes for multi-byte characters are always >= 0x80, so they can never
    // be mistaken for one of these delimiters. `start`/the found `]` index
    // therefore always land on valid `str` char boundaries.
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        let c = b as char;
        if in_string {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }

    let end = end?;
    serde_json::from_str(&response[start..=end]).ok()
}

// ---------------------------------------------------------------------
// LLM helpers
// ---------------------------------------------------------------------

fn build_prompt(base_prompt: &str, prev_summary: &str, turns: &[String]) -> String {
    format!(
        "{base_prompt}\nPrevious summary: {prev_summary}\nNew conversations:\n{}",
        turns.join("\n")
    )
}

async fn llm_complete(llm: &LlmClient, model: &str, prompt: String) -> Option<String> {
    let req = ChatRequest::new(model.to_string(), vec![ChatMessage::user(prompt)]);
    match llm.chat(req).await {
        Ok(resp) => resp.content().map(str::to_string),
        Err(err) => {
            tracing::warn!(error = %err, "npc-memory: llm completion failed");
            None
        }
    }
}

async fn run_summarize(
    llm: &LlmClient,
    chat_model: &str,
    summarize_prompt: &str,
    prev_summary: &str,
    turns: &[String],
) -> Option<String> {
    let prompt = build_prompt(summarize_prompt, prev_summary, turns);
    llm_complete(llm, chat_model, prompt).await
}

#[allow(clippy::too_many_arguments)]
async fn consolidate_long_term(
    llm: &LlmClient,
    embed_llm: &LlmClient,
    chat_model: &str,
    embedding_model: &str,
    consolidate_prompt: &str,
    prev_summary: &str,
    turns: &[String],
    chunk_size: usize,
    chunk_overlap: usize,
    store: &Arc<Mutex<VectorStore>>,
    ctx: &ModuleCtx,
    speaker_person_id: Option<&str>,
) {
    let prompt = build_prompt(consolidate_prompt, prev_summary, turns);
    let Some(consolidated) = llm_complete(llm, chat_model, prompt).await else {
        return;
    };
    if consolidated.trim().is_empty() {
        return;
    }

    let chunks = store::split_text(&consolidated, chunk_size, chunk_overlap);
    let embeddings = match embed_llm.embed(embedding_model, chunks.clone()).await {
        Ok(embeddings) => embeddings,
        Err(err) => {
            tracing::warn!(error = %err, "npc-memory: failed to embed consolidated long-term memory");
            return;
        }
    };
    if embeddings.len() != chunks.len() {
        tracing::warn!(
            expected = chunks.len(),
            got = embeddings.len(),
            "npc-memory: embedding count mismatch, skipping store"
        );
        return;
    }

    let doc_id = uuid::Uuid::new_v4().to_string();
    let created_at = chrono::Utc::now().timestamp();
    let mut metadata = HashMap::new();
    metadata.insert("type".to_string(), msg::LONG_TERM_MEMORY.to_string());
    metadata.insert("timestamp".to_string(), chrono::Utc::now().to_rfc3339());
    // Tag the chunk with the current speaker, if any, so a later
    // `VectorStore::search_for_person` for that person can favor it.
    if let Some(person_id) = speaker_person_id {
        metadata.insert("person_id".to_string(), person_id.to_string());
    }

    {
        let mut store = store.lock().await;
        for (chunk, embedding) in chunks.into_iter().zip(embeddings) {
            store.add_chunk(&doc_id, &chunk, embedding, metadata.clone(), created_at);
        }
        if let Err(err) = store.save() {
            tracing::warn!(error = %err, "npc-memory: failed to save memory store");
        }
    }

    ctx.bus.publish(topic::MEM, msg::LONG_TERM_MEMORY, json!({ "content": consolidated }));
}

#[allow(clippy::too_many_arguments)]
async fn handle_sense(
    content: String,
    llm: Arc<LlmClient>,
    embedding_model: String,
    store: Arc<Mutex<VectorStore>>,
    bus: Bus,
    top_k: usize,
    threshold: f32,
    speaker_person_id: Option<String>,
) {
    let embeddings = match llm.embed(&embedding_model, vec![content]).await {
        Ok(embeddings) => embeddings,
        Err(err) => {
            tracing::warn!(error = %err, "npc-memory: failed to embed sense query for RAG search");
            return;
        }
    };
    let Some(query_embedding) = embeddings.into_iter().next() else {
        return;
    };

    let hits = {
        let store = store.lock().await;
        store.search_for_person(&query_embedding, top_k, threshold, speaker_person_id.as_deref())
    };
    if hits.is_empty() {
        return;
    }

    let combined = hits.into_iter().map(|hit| hit.text).collect::<Vec<_>>().join("\n");
    bus.publish(topic::MEM, msg::LONG_TERM_MEMORY, json!({ "content": combined }));
}

/// Person-fact extraction: run `extract_prompt` over the just-flushed chat
/// `turns` (same shape as [`build_prompt`], minus the previous-summary
/// line — facts should come from what was actually said, not carried
/// context), parse the response as a `[{"name","facts":[...]}]` array via
/// [`parse_person_facts_response`], and merge each entry into the matching
/// (or newly created) person via [`PersonTracker::observe_from_chat`] +
/// [`PersonTracker::add_facts`], persisting/publishing each touched person.
/// A parse failure is logged and swallowed — this must never be able to
/// disrupt the conversation.
async fn extract_person_facts(
    llm: &LlmClient,
    chat_model: &str,
    extract_prompt: &str,
    turns: &[String],
    tracker: &mut PersonTracker,
    bus: &Bus,
) {
    if turns.is_empty() {
        return;
    }
    let prompt = format!("{extract_prompt}\n\n{}", turns.join("\n"));
    let Some(response) = llm_complete(llm, chat_model, prompt).await else {
        return;
    };

    let Some(extracted) = parse_person_facts_response(&response) else {
        tracing::warn!(response = %response, "npc-memory: failed to parse person fact extraction response, skipping");
        return;
    };

    for entry in extracted {
        if entry.name.trim().is_empty() || entry.facts.is_empty() {
            continue;
        }
        let Some(person_id) = tracker.observe_from_chat(&entry.name).map(|p| p.id.clone()) else {
            continue;
        };
        let facts = entry.facts.into_iter().map(|text| (text, "chat".to_string())).collect();
        tracker.add_facts(&person_id, facts);
        tracker.persist_and_publish(bus, &person_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_chat_log_formats_turn() {
        let payload = json!({"input": "hi", "output": "hello there"});
        assert_eq!(parse_chat_log(&payload), Some("User: hi\nAI: hello there".to_string()));
    }

    #[test]
    fn parse_chat_log_rejects_empty() {
        let payload = json!({});
        assert_eq!(parse_chat_log(&payload), None);
    }

    // The payload shapes below mirror `npc_talk::engine::chat_log_payload`
    // one-to-one: `{input, output, time, speaker?}` with `speaker` omitted
    // (not `null`) when absent. If that function's shape changes, these
    // tests should fail and flag the break in the cross-crate contract.

    #[test]
    fn build_chat_log_entry_copies_all_fields_with_speaker() {
        let payload = json!({
            "input": "hi",
            "output": "hello there",
            "time": "2026-07-26T00:00:00+00:00",
            "speaker": "太郎",
        });
        let entry = build_chat_log_entry(&payload).unwrap();
        assert_eq!(entry.input, "hi");
        assert_eq!(entry.output, "hello there");
        assert_eq!(entry.time, "2026-07-26T00:00:00+00:00");
        assert_eq!(entry.speaker, Some("太郎".to_string()));
    }

    #[test]
    fn build_chat_log_entry_speaker_absent_is_none() {
        let payload = json!({
            "input": "hi",
            "output": "hello there",
            "time": "2026-07-26T00:00:00+00:00",
        });
        let entry = build_chat_log_entry(&payload).unwrap();
        assert_eq!(entry.speaker, None);
    }

    #[test]
    fn build_chat_log_entry_missing_time_falls_back_to_now_rfc3339() {
        let payload = json!({"input": "hi", "output": "hello there"});
        let entry = build_chat_log_entry(&payload).unwrap();
        assert!(!entry.time.is_empty());
        assert!(chrono::DateTime::parse_from_rfc3339(&entry.time).is_ok());
    }

    #[test]
    fn build_chat_log_entry_rejects_empty_input_and_output() {
        let payload = json!({"time": "2026-07-26T00:00:00+00:00"});
        assert!(build_chat_log_entry(&payload).is_none());
    }

    #[test]
    fn parse_sense_content_bare_string() {
        let payload = json!("hello world");
        assert_eq!(parse_sense_content(&payload), Some("hello world".to_string()));
    }

    #[test]
    fn parse_sense_content_object() {
        let payload = json!({"content": "a screenshot description"});
        assert_eq!(parse_sense_content(&payload), Some("a screenshot description".to_string()));
    }

    #[test]
    fn parse_sense_content_empty_is_none() {
        assert_eq!(parse_sense_content(&json!("")), None);
        assert_eq!(parse_sense_content(&json!({"content": ""})), None);
        assert_eq!(parse_sense_content(&json!({})), None);
    }

    #[test]
    fn build_prompt_includes_summary_and_turns() {
        let prompt = build_prompt("Summarize:", "prev", &["turn1".to_string(), "turn2".to_string()]);
        assert!(prompt.contains("Summarize:"));
        assert!(prompt.contains("prev"));
        assert!(prompt.contains("turn1\nturn2"));
    }

    #[test]
    fn parse_speaker_reads_trimmed_field_and_ignores_blank() {
        assert_eq!(parse_speaker(&json!({"speaker": " 太郎 "})), Some("太郎".to_string()));
        assert_eq!(parse_speaker(&json!({"speaker": ""})), None);
        assert_eq!(parse_speaker(&json!({"content": "hi"})), None);
        assert_eq!(parse_speaker(&json!({"speaker": 123})), None);
    }

    #[test]
    fn parse_person_seen_defaults_missing_fields() {
        let seen = parse_person_seen(&json!({"name": "太郎", "appearance": "青いパーカー"})).unwrap();
        assert_eq!(seen.name, "太郎");
        assert_eq!(seen.appearance, "青いパーカー");
        assert_eq!(seen.note, "");

        // A completely empty object still parses (all fields default).
        let empty = parse_person_seen(&json!({})).unwrap();
        assert_eq!(empty.name, "");
    }

    #[test]
    fn parse_person_facts_response_handles_bare_json() {
        let response = r#"[{"name":"太郎","facts":["コーヒーが好き"]}]"#;
        let parsed = parse_person_facts_response(response).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "太郎");
        assert_eq!(parsed[0].facts, vec!["コーヒーが好き".to_string()]);
    }

    #[test]
    fn parse_person_facts_response_strips_code_fence_and_prose() {
        let response = "以下が抽出結果です。\n```json\n[{\"name\": \"花子\", \"facts\": [\"犬を飼っている\", \"猫アレルギー\"]}]\n```\nご確認ください。";
        let parsed = parse_person_facts_response(response).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "花子");
        assert_eq!(parsed[0].facts.len(), 2);
    }

    #[test]
    fn parse_person_facts_response_handles_empty_array() {
        let parsed = parse_person_facts_response("特に新しい情報はありません: []").unwrap();
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_person_facts_response_ignores_brackets_inside_strings() {
        // A fact text containing literal `[`/`]` must not confuse the
        // bracket-depth scan into ending the array early.
        let response = r#"prefix [{"name":"太郎","facts":["リスト[1]の項目"]}] suffix"#;
        let parsed = parse_person_facts_response(response).unwrap();
        assert_eq!(parsed[0].facts, vec!["リスト[1]の項目".to_string()]);
    }

    #[test]
    fn parse_person_facts_response_none_when_no_array_present() {
        assert!(parse_person_facts_response("no json here").is_none());
        assert!(parse_person_facts_response("").is_none());
    }
}
