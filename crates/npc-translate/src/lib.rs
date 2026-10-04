//! Simultaneous interpretation module (ports Go `agent-speech`'s translation
//! feature — `internal/app/app.go`'s `handleTranslation` /
//! `handleAgentTranslation`, driven there by the `translation` config
//! section).
//!
//! Bus wiring: subscribes `agent:sense` (`speech`) for what the NPC hears and
//! `agent:chat` (`chat_response`) for what it answers, and publishes one
//! `translation` message per result on `npc:ui`, which `npc-server` forwards
//! to the web UI's 通訳 tab. Optionally also writes subtitles to the VRChat
//! chatbox over OSC (`translation.chatbox`).
//!
//! `config.translation.mode` decides what runs:
//!
//! * `off` — nothing; the module idles on the bus until a config update.
//! * `interpret` — heard speech is translated, and **npc-talk stays silent**
//!   (npc-talk reads the same setting; see its `module.rs`). This is the
//!   Go original's "translation mode": the NPC is an interpreter, not a
//!   conversation partner.
//! * `assist` — normal conversation, with both sides subtitled.
//!
//! `config.translation.scope` then picks which halves of that actually get
//! translated — `input` (heard speech only), `output` (the NPC's replies
//! only) or `both`, the default and the only behaviour this module had before
//! the setting existed. It is a separate axis from the mode on purpose: the
//! mode decides whether the NPC answers, the scope decides what gets
//! subtitled. Not every pairing does something — `interpret` + `output` asks
//! for subtitles on replies that mode has already suppressed, and translates
//! nothing — which the web UI warns about at the point of choosing.
//!
//! Like npc-scheduler this module is always spawned and re-reads its settings
//! from [`npc_core::bus::msg::CONFIG_UPDATED`] on
//! [`npc_core::bus::topic::CONFIG`], so the mode and target languages can be
//! switched from the web UI without a restart. The LLM *connection*
//! (`api.base_url` / `api.api_key`) is read once at startup, matching every
//! other module.

mod engine;
mod langdetect;

use std::sync::Arc;

use async_trait::async_trait;
use npc_core::config::{TranslationMode, TranslationScope};
use npc_core::{msg, topic, BusMessage, Config, Module, ModuleCtx};
use tokio::sync::broadcast::error::RecvError;

use engine::{Source, TranslationEngine};

pub fn module(_ctx: &ModuleCtx) -> anyhow::Result<Box<dyn Module>> {
    Ok(Box::new(TranslateModule))
}

struct TranslateModule;

#[async_trait]
impl Module for TranslateModule {
    fn name(&self) -> &'static str {
        "npc-translate"
    }

    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()> {
        let engine = Arc::new(TranslationEngine::new(ctx.bus.clone(), &ctx.config)?);
        log_settings(&engine, &ctx.config);

        let mut rx = ctx.bus.subscribe();

        loop {
            tokio::select! {
                _ = ctx.shutdown.cancelled() => {
                    tracing::info!("npc-translate: shutting down");
                    return Ok(());
                }
                received = rx.recv() => {
                    match received {
                        Ok(bus_msg) => handle_bus_message(&engine, bus_msg),
                        Err(RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "npc-translate: bus receiver lagged, some messages dropped");
                        }
                        Err(RecvError::Closed) => {
                            tracing::info!("npc-translate: bus closed, shutting down");
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

fn log_settings(engine: &TranslationEngine, config: &Config) {
    let scope = engine.scope();
    match engine.mode() {
        TranslationMode::Off => tracing::info!("npc-translate: mode=off, idling until config update"),
        // Worth its own line rather than a footnote on the active log: this
        // pairing is configured, valid, and translates nothing, so the log
        // has to say so or it looks like a bug at the first utterance.
        TranslationMode::Interpret if scope == TranslationScope::Output => tracing::warn!(
            "npc-translate: scope=output with mode=interpret — the NPC does not reply in this mode, so nothing will be translated"
        ),
        mode => tracing::info!(
            mode = ?mode,
            scope = ?scope,
            source = %config.translation.source_language,
            targets = ?config.translation.target_languages(),
            "npc-translate: interpretation active"
        ),
    }
}

fn handle_bus_message(engine: &Arc<TranslationEngine>, bus_msg: BusMessage) {
    match bus_msg.topic.as_str() {
        t if t == topic::CONFIG => {
            if bus_msg.env.r#type == msg::CONFIG_UPDATED {
                match serde_json::from_value::<Config>(bus_msg.env.payload) {
                    Ok(new_config) => {
                        engine.update_settings(&new_config);
                        tracing::info!("npc-translate: config updated");
                        log_settings(engine, &new_config);
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "npc-translate: failed to deserialize updated config, keeping current settings");
                    }
                }
            }
        }
        t if t == topic::SENSE => {
            // The input half. `scope` is the second gate: `output` means the
            // user only wants the NPC's replies subtitled, so heard speech
            // is left alone even though the mode is on.
            if bus_msg.env.r#type == msg::SPEECH
                && !engine.mode().is_off()
                && engine.scope().translates_input()
            {
                spawn_translation(engine, Source::User, &bus_msg.env.payload);
            }
        }
        t if t == topic::CHAT => {
            // The output half. Only `assist` subtitles the NPC's own replies
            // — in `interpret` mode npc-talk isn't answering in the first
            // place — and only when `scope` asks for that half.
            if bus_msg.env.r#type == msg::CHAT_RESPONSE
                && engine.mode() == TranslationMode::Assist
                && engine.scope().translates_output()
            {
                spawn_translation(engine, Source::Agent, &bus_msg.env.payload);
            }
        }
        _ => {}
    }
}

fn spawn_translation(engine: &Arc<TranslationEngine>, source: Source, payload: &serde_json::Value) {
    let Some(text) = extract_content(payload) else {
        return;
    };
    if text.trim().is_empty() {
        return;
    }
    let engine = engine.clone();
    tokio::spawn(async move {
        engine.translate(source, text).await;
    });
}

/// Payload may be `{"content": "..."}` or a bare JSON string, matching how
/// npc-talk reads the same topics.
fn extract_content(payload: &serde_json::Value) -> Option<String> {
    if let Some(s) = payload.as_str() {
        return Some(s.to_string());
    }
    payload.get("content").and_then(|v| v.as_str()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_content_from_both_payload_shapes() {
        assert_eq!(extract_content(&json!("hi")), Some("hi".to_string()));
        assert_eq!(extract_content(&json!({"content": "hi"})), Some("hi".to_string()));
        assert_eq!(extract_content(&json!({"other": "hi"})), None);
    }
}
