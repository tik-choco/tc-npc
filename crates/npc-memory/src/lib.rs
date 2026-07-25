//! Memory module (ports Go `agent-memory`). Maintains a rolling short-term
//! summary of recent chat turns and a long-term vector store, driven by the
//! in-process bus instead of Redis pub/sub.
//!
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
use npc_core::{Module, ModuleCtx};
use npc_llm::{ChatMessage, ChatRequest, LlmClient};

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
        let llm = Arc::new(
            LlmClient::new(cfg.api.base_url.clone(), cfg.api.api_key.clone())
                .with_reasoning_effort(cfg.api.reasoning_effort.clone()),
        );
        let chat_model = cfg.api.model.clone();
        let embedding_model = cfg.api.embedding_model.clone();

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
                                &chat_model,
                                &embedding_model,
                                &consolidate_prompt,
                                &prev_summary,
                                &turns,
                                cfg.memory.chunk_size as usize,
                                cfg.memory.chunk_overlap as usize,
                                &store,
                                &ctx,
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
                                    if embedding_model.is_empty() {
                                        continue;
                                    }
                                    if let Some(content) = parse_sense_content(&bus_msg.env.payload) {
                                        tokio::spawn(handle_sense(
                                            content,
                                            llm.clone(),
                                            embedding_model.clone(),
                                            store.clone(),
                                            ctx.bus.clone(),
                                            cfg.memory.top_k as usize,
                                            cfg.memory.threshold,
                                        ));
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
    chat_model: &str,
    embedding_model: &str,
    consolidate_prompt: &str,
    prev_summary: &str,
    turns: &[String],
    chunk_size: usize,
    chunk_overlap: usize,
    store: &Arc<Mutex<VectorStore>>,
    ctx: &ModuleCtx,
) {
    let prompt = build_prompt(consolidate_prompt, prev_summary, turns);
    let Some(consolidated) = llm_complete(llm, chat_model, prompt).await else {
        return;
    };
    if consolidated.trim().is_empty() {
        return;
    }

    let chunks = store::split_text(&consolidated, chunk_size, chunk_overlap);
    let embeddings = match llm.embed(embedding_model, chunks.clone()).await {
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

async fn handle_sense(
    content: String,
    llm: Arc<LlmClient>,
    embedding_model: String,
    store: Arc<Mutex<VectorStore>>,
    bus: npc_core::Bus,
    top_k: usize,
    threshold: f32,
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
        store.search(&query_embedding, top_k, threshold)
    };
    if hits.is_empty() {
        return;
    }

    let combined = hits.into_iter().map(|hit| hit.text).collect::<Vec<_>>().join("\n");
    bus.publish(topic::MEM, msg::LONG_TERM_MEMORY, json!({ "content": combined }));
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
}
