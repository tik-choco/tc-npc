//! Vision module (ports Go `agent-vision`): periodic screen/window capture
//! and VLM ("what do you see") description, with an optional single robot
//! action tool call per cycle.

mod capture;
mod vlm;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use npc_core::{msg, topic, Module, ModuleCtx};
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

    let base_url = if !config.vision.base_url.is_empty() {
        config.vision.base_url.clone()
    } else {
        config.api.base_url.clone()
    };
    let api_key = if !config.vision.api_key.is_empty() {
        config.vision.api_key.clone()
    } else {
        config.api.api_key.clone()
    };
    let model = if !config.vision.model.is_empty() {
        config.vision.model.clone()
    } else {
        config.api.model.clone()
    };

    let client = LlmClient::new(base_url, api_key);

    // Single-flight loop: capture + analyze, then sleep `interval_seconds`
    // measured from the end of the previous cycle, until shutdown.
    loop {
        if shutdown.is_cancelled() {
            break;
        }

        run_cycle(&client, &model, &config.vision, &data_dir, &bus, &memory_context).await;

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
        &vision_config.system_prompt,
        data_url,
        mem_ctx.as_deref(),
        bus,
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
