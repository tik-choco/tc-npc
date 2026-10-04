//! Vision module (ports Go `agent-vision`): periodic screen/window capture
//! and VLM ("what do you see") description, with an optional single robot
//! action tool call per cycle.

mod capture;
mod vlm;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use npc_core::{msg, topic, LlmTask, Module, ModuleCtx};
use npc_llm::LlmClient;
use tokio::sync::Mutex;
use tokio::sync::broadcast::error::RecvError;

pub fn module(_ctx: &ModuleCtx) -> anyhow::Result<Box<dyn Module>> {
    Ok(Box::new(VisionModule))
}

struct VisionModule;

#[async_trait]
impl Module for VisionModule {
    fn name(&self) -> &'static str {
        "npc-vision"
    }

    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()> {
        run_vision(ctx).await
    }
}

async fn run_vision(ctx: ModuleCtx) -> anyhow::Result<()> {
    let ModuleCtx {
        bus,
        config,
        shutdown,
        data_dir,
        ..
    } = ctx;

    // Latest short-term memory context, updated by `agent:sense`/`speech`
    // messages, read by each VLM cycle.
    let memory_context: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // Subscriber task: track short-term memory context from `speech`
    // messages on `agent:sense`, ignoring our own `vision` messages.
    {
        let mut rx = bus.subscribe();
        let memory_context = memory_context.clone();
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    received = rx.recv() => {
                        match received {
                            Ok(bus_msg) => {
                                if bus_msg.topic != topic::SENSE || bus_msg.env.r#type != msg::SPEECH {
                                    continue;
                                }
                                let payload = &bus_msg.env.payload;
                                let content = payload
                                    .get("content")
                                    .and_then(|v| v.as_str())
                                    .map(str::to_string)
                                    .or_else(|| payload.as_str().map(str::to_string));
                                if let Some(content) = content {
                                    *memory_context.lock().await = Some(content);
                                }
                            }
                            Err(RecvError::Lagged(_)) => continue,
                            Err(RecvError::Closed) => break,
                        }
                    }
                }
            }
        });
    }

    // 接続先・モデルの解決は npc-core の resolve_llm に一本化した(旧: vision.*
    // が空なら api.* にフォールバックする自前ロジック)。
    let resolved = config.resolve_llm(LlmTask::Vision)?;
    let model = resolved.model;

    let client = LlmClient::new(resolved.base_url, resolved.api_key)
        .with_reasoning_effort(resolved.reasoning_effort);

    // When person detection is on, tell the model about `observe_person`
    // *before* applying the language instruction rather than after: the
    // language instruction ("Always write your reply in ...") reads like the
    // final word on output format, so appending after it would visually bury
    // the person-detection instruction past what looks like the prompt's
    // closing line. Folding it into `system_prompt` first keeps everything
    // about *what to do* together, with the language directive staying last.
    let mut system_prompt_base = config.vision.system_prompt.clone();
    if config.vision.person_detection {
        system_prompt_base
            .push_str("\n\n画面に人物が写っている場合は observe_person ツールで記録してください。");
    }

    // The observation text is published on `agent:sense` and read back by
    // npc-talk/npc-memory, so it follows `config.language` like every other
    // LLM output. Resolved once — the prompt can't change without a restart.
    let system_prompt =
        npc_core::config::with_language_instruction(&system_prompt_base, &config.language);

    // Single-flight loop: capture + analyze, then sleep `interval_seconds`
    // measured from the end of the previous cycle, until shutdown.
    loop {
        if shutdown.is_cancelled() {
            break;
        }

        run_cycle(
            &client,
            &model,
            &config.vision,
            &system_prompt,
            &data_dir,
            &bus,
            &memory_context,
        )
        .await;

        let interval_secs = config.vision.interval_seconds.max(0.1);
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_secs_f32(interval_secs)) => {}
        }
    }

    Ok(())
}

async fn run_cycle(
    client: &LlmClient,
    model: &str,
    vision_config: &npc_core::config::VisionConfig,
    // `vision_config.system_prompt` with the `config.language` instruction
    // already applied.
    system_prompt: &str,
    data_dir: &std::path::Path,
    bus: &npc_core::Bus,
    memory_context: &Arc<Mutex<Option<String>>>,
) {
    tracing::debug!("vision: starting capture cycle");

    let img = match capture::capture(vision_config, data_dir) {
        Ok(img) => img,
        Err(err) => {
            tracing::warn!(error = %err, "vision: capture failed");
            return;
        }
    };

    let data_url = match capture::encode_data_url(&img) {
        Ok(url) => url,
        Err(err) => {
            tracing::warn!(error = %err, "vision: failed to encode captured image");
            return;
        }
    };

    let mem_ctx = memory_context.lock().await.clone();

    match vlm::analyze_image(
        client,
        model,
        system_prompt,
        data_url,
        mem_ctx.as_deref(),
        bus,
        vision_config.person_detection,
    )
    .await
    {
        Ok(text) => {
            tracing::info!(text = %text, "vision: cycle complete");
            bus.publish(topic::SENSE, msg::VISION, serde_json::json!({ "content": text }));
        }
        Err(err) => {
            tracing::warn!(error = %err, "vision: VLM analysis failed");
        }
    }
}
