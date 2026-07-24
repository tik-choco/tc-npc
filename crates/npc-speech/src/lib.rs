//! Speech module (ports Go `agent-speech`): mic capture + RMS VAD
//! segmentation + OpenAI-compatible STT, and bus-driven OpenAI-compatible
//! TTS playback, wired independently by `config.stt.enabled` /
//! `config.tts.enabled`.
//!
//! cpal input/output streams are `!Send`, so all blocking audio I/O lives
//! on two dedicated `std::thread`s (capture + playback) that never touch
//! the tokio runtime directly; they communicate with async bus-subscriber
//! tasks over channels. See `capture.rs` / `playback.rs` for the per-thread
//! detail and `vad.rs` for the segmentation state machine.

mod capture;
mod device;
mod osc;
mod playback;
mod resample;
mod vad;
mod wavio;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc};
use std::time::Duration;

use async_trait::async_trait;
use npc_core::config::{ApiConfig, Config};
use npc_core::{Module, ModuleCtx};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::UnboundedReceiver;

use capture::{run_capture_thread, CaptureEvent};
use playback::{run_playback_thread, PlaybackCmd};

/// Bus topic the web UI listens on for the mic VU meter. Not part of
/// `npc_core::bus::topic` since it's a UI-facing signal, not an
/// agent-to-agent one.
const UI_TOPIC: &str = "npc:ui";
const UI_MSG_VOLUME: &str = "volume";

/// How long a `suspend` (from `agent:interrupt`) lasts before auto-resuming
/// if no explicit `resume` arrives (Go: `SuspendTimeout`).
const SUSPEND_TIMEOUT: Duration = Duration::from_secs(600);

pub fn module(_ctx: &ModuleCtx) -> anyhow::Result<Box<dyn Module>> {
    Ok(Box::new(SpeechModule))
}

struct SpeechModule;

/// Tracks suspend/resume state for `chat_response`-driven TTS, with a
/// generation counter so a stale timeout doesn't clobber a more recent
/// resume (ports the Go original's `suspendGen` pattern).
struct SuspendState {
    suspended: AtomicBool,
    generation: AtomicU64,
}

#[async_trait]
impl Module for SpeechModule {
    fn name(&self) -> &'static str {
        "npc-speech"
    }

    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()> {
        let cfg = ctx.config.clone();

        if !cfg.stt.enabled && !cfg.tts.enabled {
            tracing::info!("npc-speech: stt and tts both disabled, module idling");
            ctx.shutdown.cancelled().await;
            return Ok(());
        }

        // Shared "TTS is actively producing audio right now" flag: the
        // playback thread sets it, the capture thread reads it to suppress
        // mic->STT while the agent is speaking (echo prevention).
        let is_tts_playing = Arc::new(AtomicBool::new(false));

        let mut thread_handles: Vec<std::thread::JoinHandle<()>> = Vec::new();

        // --- TTS: playback thread + bus subscriber tasks -----------------
        let playback_tx: Option<std_mpsc::Sender<PlaybackCmd>> = if cfg.tts.enabled {
            let (tx, rx) = std_mpsc::channel::<PlaybackCmd>();
            let speech_cfg = cfg.speech.clone();
            let flag = is_tts_playing.clone();
            let handle = std::thread::Builder::new()
                .name("npc-speech-playback".into())
                .spawn(move || run_playback_thread(speech_cfg, rx, flag))
                .expect("npc-speech: failed to spawn playback thread");
            thread_handles.push(handle);

            let suspend = Arc::new(SuspendState {
                suspended: AtomicBool::new(false),
                generation: AtomicU64::new(0),
            });

            tokio::spawn(run_chat_response_tts_task(ctx.clone(), tx.clone(), suspend.clone()));
            tokio::spawn(run_interrupt_task(ctx.clone(), tx.clone(), suspend));

            Some(tx)
        } else {
            None
        };

        // --- STT: capture thread + segment/volume consumer task ----------
        let capture_stop = Arc::new(AtomicBool::new(false));
        if cfg.stt.enabled {
            let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel::<CaptureEvent>();
            let speech_cfg = cfg.speech.clone();
            let stt_cfg = cfg.stt.clone();
            let flag = is_tts_playing.clone();
            let stop = capture_stop.clone();
            let handle = std::thread::Builder::new()
                .name("npc-speech-capture".into())
                .spawn(move || run_capture_thread(speech_cfg, stt_cfg, events_tx, stop, flag))
                .expect("npc-speech: failed to spawn capture thread");
            thread_handles.push(handle);

            tokio::spawn(run_capture_events_task(ctx.clone(), events_rx));
        }

        ctx.shutdown.cancelled().await;
        tracing::info!("npc-speech: shutting down");

        capture_stop.store(true, Ordering::SeqCst);
        if let Some(tx) = &playback_tx {
            let _ = tx.send(PlaybackCmd::Shutdown);
        }

        // Audio threads are blocking `std::thread`s; join them off the
        // tokio worker thread so shutdown doesn't stall the runtime.
        tokio::task::spawn_blocking(move || {
            for handle in thread_handles {
                let _ = handle.join();
            }
        })
        .await
        .ok();

        Ok(())
    }
}

// ---------------------------------------------------------------------
// STT: capture events -> bus
// ---------------------------------------------------------------------

async fn run_capture_events_task(ctx: ModuleCtx, mut rx: UnboundedReceiver<CaptureEvent>) {
    loop {
        let ev = tokio::select! {
            _ = ctx.shutdown.cancelled() => break,
            ev = rx.recv() => ev,
        };
        let Some(ev) = ev else { break };

        match ev {
            CaptureEvent::Volume(level) => {
                ctx.bus
                    .publish(UI_TOPIC, UI_MSG_VOLUME, serde_json::json!({ "level": level }));
            }
            CaptureEvent::Segment(wav_bytes) => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    transcribe_and_publish(&ctx, wav_bytes).await;
                });
            }
        }
    }
}

async fn transcribe_and_publish(ctx: &ModuleCtx, wav_bytes: Vec<u8>) {
    let cfg = &ctx.config;
    let client = build_llm_client(&cfg.stt.base_url, &cfg.stt.api_key, &cfg.api);

    let text = match client.transcribe(&cfg.stt.model, wav_bytes).await {
        Ok(text) => text,
        Err(err) => {
            tracing::error!(error = %err, "npc-speech: stt transcription failed");
            return;
        }
    };

    let text = text.trim();
    if text.is_empty() {
        return;
    }

    ctx.bus.publish(
        npc_core::topic::SENSE,
        npc_core::msg::SPEECH,
        serde_json::json!({ "content": text }),
    );

    if cfg.vrc.chatbox {
        let addr = cfg.vrc.osc_address.clone();
        let text = text.to_string();
        tokio::task::spawn_blocking(move || osc::send_chatbox(&addr, &text));
    }
}

// ---------------------------------------------------------------------
// TTS: bus -> playback
// ---------------------------------------------------------------------

async fn run_chat_response_tts_task(
    ctx: ModuleCtx,
    playback_tx: std_mpsc::Sender<PlaybackCmd>,
    suspend: Arc<SuspendState>,
) {
    let mut rx = ctx.bus.subscribe();
    loop {
        let recv = tokio::select! {
            _ = ctx.shutdown.cancelled() => break,
            r = rx.recv() => r,
        };

        let msg = match recv {
            Ok(m) => m,
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "npc-speech: bus receiver lagged (chat_response)");
                continue;
            }
            Err(RecvError::Closed) => break,
        };

        if msg.topic != npc_core::topic::CHAT || msg.env.r#type != npc_core::msg::CHAT_RESPONSE {
            continue;
        }

        if suspend.suspended.load(Ordering::SeqCst) {
            tracing::info!("npc-speech: suspended, dropping chat_response tts");
            continue;
        }

        let Some(content) = msg.env.payload.get("content").and_then(|v| v.as_str()) else {
            continue;
        };
        if content.is_empty() {
            continue;
        }

        let cfg = ctx.config.clone();
        let content = content.to_string();
        let tx = playback_tx.clone();

        tokio::spawn(async move {
            let text = truncate_for_tts(&content, cfg.tts.max_len as usize);
            if let Some(wav) = synthesize_tts(&cfg, &text).await {
                if cfg.vrc.chatbox {
                    let addr = cfg.vrc.osc_address.clone();
                    let text = text.clone();
                    tokio::task::spawn_blocking(move || osc::send_chatbox(&addr, &text));
                }
                let _ = tx.send(PlaybackCmd::Enqueue(wav));
            }
        });
    }
}

async fn run_interrupt_task(ctx: ModuleCtx, playback_tx: std_mpsc::Sender<PlaybackCmd>, suspend: Arc<SuspendState>) {
    let mut rx = ctx.bus.subscribe();
    loop {
        let recv = tokio::select! {
            _ = ctx.shutdown.cancelled() => break,
            r = rx.recv() => r,
        };

        let msg = match recv {
            Ok(m) => m,
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "npc-speech: bus receiver lagged (interrupt)");
                continue;
            }
            Err(RecvError::Closed) => break,
        };

        if msg.topic != npc_core::topic::INTERRUPT {
            continue;
        }

        match msg.env.r#type.as_str() {
            t if t == npc_core::msg::TTS => {
                let content = msg
                    .env
                    .payload
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let chime_file = msg
                    .env
                    .payload
                    .get("chime_file")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if content.is_empty() && chime_file.is_empty() {
                    continue;
                }

                let cfg = ctx.config.clone();
                let tx = playback_tx.clone();
                tokio::spawn(async move {
                    handle_priority_tts(&cfg, content, chime_file, tx).await;
                });
            }
            t if t == npc_core::msg::SUSPEND => {
                suspend.suspended.store(true, Ordering::SeqCst);
                let gen = suspend.generation.fetch_add(1, Ordering::SeqCst) + 1;
                let _ = playback_tx.send(PlaybackCmd::Stop);
                tracing::info!("npc-speech: suspended (chat_response tts paused)");

                let suspend = suspend.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(SUSPEND_TIMEOUT).await;
                    if suspend.generation.load(Ordering::SeqCst) == gen {
                        suspend.suspended.store(false, Ordering::SeqCst);
                        tracing::info!("npc-speech: suspend timed out, auto-resuming");
                    }
                });
            }
            t if t == npc_core::msg::RESUME => {
                suspend.suspended.store(false, Ordering::SeqCst);
                suspend.generation.fetch_add(1, Ordering::SeqCst);
                tracing::info!("npc-speech: resumed");
            }
            _ => {}
        }
    }
}

async fn handle_priority_tts(cfg: &Config, content: String, chime_file: String, tx: std_mpsc::Sender<PlaybackCmd>) {
    let mut items: Vec<Vec<u8>> = Vec::new();

    if !chime_file.is_empty() {
        match tokio::fs::read(&chime_file).await {
            Ok(bytes) => items.push(bytes),
            Err(err) => {
                tracing::warn!(error = %err, file = %chime_file, "npc-speech: chime_file not found, skipping");
            }
        }
    }

    if !content.is_empty() {
        let text = truncate_for_tts(&content, cfg.tts.max_len as usize);
        if let Some(wav) = synthesize_tts(cfg, &text).await {
            if cfg.vrc.chatbox {
                osc::send_chatbox(&cfg.vrc.osc_address, &text);
            }
            items.push(wav);
        }
    }

    if !items.is_empty() {
        let _ = tx.send(PlaybackCmd::Priority(items));
    }
}

// ---------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------

async fn synthesize_tts(cfg: &Config, text: &str) -> Option<Vec<u8>> {
    let client = build_llm_client(&cfg.tts.base_url, &cfg.tts.api_key, &cfg.api);
    let model = if cfg.tts.model.is_empty() {
        cfg.api.model.clone()
    } else {
        cfg.tts.model.clone()
    };

    match client.speak(&model, &cfg.tts.voice, text, cfg.tts.speed).await {
        Ok(wav) => Some(wav),
        Err(err) => {
            tracing::error!(error = %err, "npc-speech: tts synthesis failed");
            None
        }
    }
}

fn truncate_for_tts(content: &str, max_len: usize) -> String {
    if max_len == 0 || content.chars().count() <= max_len {
        return content.to_string();
    }
    let mut truncated: String = content.chars().take(max_len).collect();
    truncated.push_str("...");
    truncated
}

/// `base_url`/`api_key` empty falls back to `config.api.*`, per the STT/TTS
/// config doc comments.
fn build_llm_client(base_url: &str, api_key: &str, fallback: &ApiConfig) -> npc_llm::LlmClient {
    let base = if base_url.is_empty() {
        fallback.base_url.clone()
    } else {
        base_url.to_string()
    };
    let key = if api_key.is_empty() {
        fallback.api_key.clone()
    } else {
        api_key.to_string()
    };
    npc_llm::LlmClient::new(base, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_for_tts_by_char_count() {
        let long = "a".repeat(250);
        let t = truncate_for_tts(&long, 200);
        assert_eq!(t.chars().count(), 203); // 200 + "..."
    }

    #[test]
    fn leaves_short_text_alone() {
        assert_eq!(truncate_for_tts("hello", 200), "hello");
    }
}
