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
//!
//! Unlike most modules this one is always started, and the two audio threads
//! come and go with the config: a `npc:config` `config_updated` that flips
//! `stt.enabled` / `tts.enabled`, or names a different device, restarts just
//! the affected thread. Toggling the mic from the web UI used to need an app
//! restart, which made the cascade look broken rather than switched off.

mod capture;
mod device;
mod playback;
mod resample;
mod vad;
mod wavio;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use npc_core::config::{Config, SpeechConfig};
use npc_core::osc;
use npc_core::{LlmTask, Module, ModuleCtx};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use capture::{run_capture_thread, CaptureEvent};
use playback::{run_playback_thread, PlaybackCmd};
use vad::VadParams;

/// Host audio device enumeration, re-exported for npc-server's
/// `GET /api/audio/devices` (the 音声 panel's device pickers). This is the
/// only part of the module usable without starting it — it opens no stream
/// and doesn't need the module to be enabled.
pub use device::{list_devices, AudioDevices};

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

/// State the long-lived bus tasks share with the audio threads, which are
/// started and stopped underneath them.
///
/// The tasks are spawned once and outlive every capture/playback thread, so
/// they can't hold a `Sender` directly — `playback_tx` is the slot the
/// currently-running playback thread publishes itself into, and `None` means
/// TTS is switched off right now.
struct Shared {
    /// Latest config seen on `npc:config`, so endpoint/model/voice edits take
    /// effect on the next request instead of at the next app start.
    config: RwLock<Arc<Config>>,
    vad_params: Arc<VadParams>,
    /// Set by the playback thread while a clip is audible; read by the
    /// capture thread for echo prevention and barge-in.
    is_tts_playing: Arc<AtomicBool>,
    /// Narrows the above to clips barge-in must not cut (chimes, scheduler
    /// announcements).
    is_priority_playing: Arc<AtomicBool>,
    /// The web UI's 音声 開始/停止 master switch.
    voice_active: Arc<AtomicBool>,
    playback_tx: Mutex<Option<std_mpsc::Sender<PlaybackCmd>>>,
}

impl Shared {
    fn config(&self) -> Arc<Config> {
        self.config.read().unwrap().clone()
    }

    fn send_playback(&self, cmd: PlaybackCmd) {
        if let Some(tx) = self.playback_tx.lock().unwrap().as_ref() {
            let _ = tx.send(cmd);
        }
    }
}

/// A running audio thread, held so the config loop can stop and join it.
struct PlaybackThread {
    tx: std_mpsc::Sender<PlaybackCmd>,
    handle: std::thread::JoinHandle<()>,
    /// The `speech` section it was opened with, to spot a device change.
    speech: SpeechConfig,
}

struct CaptureThread {
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
    speech: SpeechConfig,
}

/// Which audio threads the config asks for, and with what device settings.
/// Both halves are compared against what is actually running to decide
/// whether to start, stop, or restart a thread.
fn playback_needs_restart(running: Option<&PlaybackThread>, cfg: &Config) -> bool {
    match running {
        Some(t) => t.speech.output_device != cfg.speech.output_device,
        None => false,
    }
}

fn capture_needs_restart(running: Option<&CaptureThread>, cfg: &Config) -> bool {
    match running {
        Some(t) => {
            t.speech.input_device != cfg.speech.input_device
                || t.speech.input_sample_rate != cfg.speech.input_sample_rate
        }
        None => false,
    }
}

#[async_trait]
impl Module for SpeechModule {
    fn name(&self) -> &'static str {
        "npc-speech"
    }

    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()> {
        let shared = Arc::new(Shared {
            config: RwLock::new(ctx.config.clone()),
            vad_params: VadParams::new(&ctx.config.stt),
            is_tts_playing: Arc::new(AtomicBool::new(false)),
            is_priority_playing: Arc::new(AtomicBool::new(false)),
            // Starts on, so a run with no UI attached behaves exactly as it
            // did before the toggle existed.
            voice_active: Arc::new(AtomicBool::new(true)),
            playback_tx: Mutex::new(None),
        });

        let suspend = Arc::new(SuspendState {
            suspended: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        });

        // One capture-event channel for the module's whole lifetime: capture
        // threads come and go holding clones of the sender, while the
        // consumer task below is spawned once.
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel::<CaptureEvent>();

        tokio::spawn(run_capture_events_task(ctx.clone(), shared.clone(), events_rx));
        tokio::spawn(run_chat_response_tts_task(
            ctx.clone(),
            shared.clone(),
            suspend.clone(),
        ));
        tokio::spawn(run_interrupt_task(ctx.clone(), shared.clone(), suspend));
        tokio::spawn(run_voice_gate_task(ctx.clone(), shared.clone()));

        let mut playback: Option<PlaybackThread> = None;
        let mut capture: Option<CaptureThread> = None;
        apply_config(&shared, &events_tx, &mut playback, &mut capture, &ctx.config);

        let mut rx = ctx.bus.subscribe();
        loop {
            let recv = tokio::select! {
                _ = ctx.shutdown.cancelled() => break,
                r = rx.recv() => r,
            };

            let msg = match recv {
                Ok(m) => m,
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "npc-speech: bus receiver lagged (config)");
                    continue;
                }
                Err(RecvError::Closed) => break,
            };

            if msg.topic != npc_core::topic::CONFIG || msg.env.r#type != npc_core::msg::CONFIG_UPDATED {
                continue;
            }

            match serde_json::from_value::<Config>(msg.env.payload) {
                Ok(new_config) => {
                    let new_config = Arc::new(new_config);
                    *shared.config.write().unwrap() = new_config.clone();
                    shared.vad_params.set(&new_config.stt);
                    apply_config(&shared, &events_tx, &mut playback, &mut capture, &new_config);
                }
                Err(err) => {
                    tracing::warn!(error = %err, "npc-speech: ignoring malformed config update");
                }
            }
        }

        tracing::info!("npc-speech: shutting down");
        stop_playback(&shared, &mut playback);
        stop_capture(&mut capture);
        Ok(())
    }
}

// ---------------------------------------------------------------------
// audio thread lifecycle
// ---------------------------------------------------------------------

/// Bring the running audio threads in line with `cfg`. Called once at
/// startup and again on every `config_updated`; a no-op when nothing that
/// matters changed, so an unrelated config save doesn't cut off a reply
/// mid-sentence by re-opening the output device.
fn apply_config(
    shared: &Arc<Shared>,
    events_tx: &UnboundedSender<CaptureEvent>,
    playback: &mut Option<PlaybackThread>,
    capture: &mut Option<CaptureThread>,
    cfg: &Arc<Config>,
) {
    if !cfg.tts.enabled || playback_needs_restart(playback.as_ref(), cfg) {
        stop_playback(shared, playback);
    }
    if cfg.tts.enabled && playback.is_none() {
        start_playback(shared, playback, cfg);
    }

    if !cfg.stt.enabled || capture_needs_restart(capture.as_ref(), cfg) {
        stop_capture(capture);
    }
    if cfg.stt.enabled && capture.is_none() {
        start_capture(shared, events_tx, capture, cfg);
    }
}

fn start_playback(shared: &Arc<Shared>, slot: &mut Option<PlaybackThread>, cfg: &Arc<Config>) {
    let (tx, rx) = std_mpsc::channel::<PlaybackCmd>();
    let speech = cfg.speech.clone();
    let thread_speech = speech.clone();
    let playing = shared.is_tts_playing.clone();
    let priority = shared.is_priority_playing.clone();

    let handle = match std::thread::Builder::new()
        .name("npc-speech-playback".into())
        .spawn(move || run_playback_thread(thread_speech, rx, playing, priority))
    {
        Ok(h) => h,
        Err(err) => {
            tracing::error!(error = %err, "npc-speech: failed to spawn playback thread");
            return;
        }
    };

    *shared.playback_tx.lock().unwrap() = Some(tx.clone());
    *slot = Some(PlaybackThread { tx, handle, speech });
    tracing::info!("npc-speech: TTS playback started");
}

fn stop_playback(shared: &Arc<Shared>, slot: &mut Option<PlaybackThread>) {
    let Some(thread) = slot.take() else { return };
    // Drop the shared handle first so nothing queues onto a thread that is
    // on its way out.
    *shared.playback_tx.lock().unwrap() = None;
    let _ = thread.tx.send(PlaybackCmd::Shutdown);
    drop(thread.tx);
    let _ = thread.handle.join();
    shared.is_tts_playing.store(false, Ordering::SeqCst);
    shared.is_priority_playing.store(false, Ordering::SeqCst);
    tracing::info!("npc-speech: TTS playback stopped");
}

fn start_capture(
    shared: &Arc<Shared>,
    events_tx: &UnboundedSender<CaptureEvent>,
    slot: &mut Option<CaptureThread>,
    cfg: &Arc<Config>,
) {
    let stop = Arc::new(AtomicBool::new(false));
    let speech = cfg.speech.clone();
    let thread_speech = speech.clone();
    let params = shared.vad_params.clone();
    let events_tx = events_tx.clone();
    let thread_stop = stop.clone();
    let playing = shared.is_tts_playing.clone();
    let active = shared.voice_active.clone();

    let handle = match std::thread::Builder::new()
        .name("npc-speech-capture".into())
        .spawn(move || run_capture_thread(thread_speech, params, events_tx, thread_stop, playing, active))
    {
        Ok(h) => h,
        Err(err) => {
            tracing::error!(error = %err, "npc-speech: failed to spawn capture thread");
            return;
        }
    };

    *slot = Some(CaptureThread { stop, handle, speech });
    tracing::info!("npc-speech: mic capture started");
}

fn stop_capture(slot: &mut Option<CaptureThread>) {
    let Some(thread) = slot.take() else { return };
    thread.stop.store(true, Ordering::SeqCst);
    let _ = thread.handle.join();
    tracing::info!("npc-speech: mic capture stopped");
}

// ---------------------------------------------------------------------
// STT: capture events -> bus
// ---------------------------------------------------------------------

async fn run_capture_events_task(ctx: ModuleCtx, shared: Arc<Shared>, mut rx: UnboundedReceiver<CaptureEvent>) {
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
                let shared = shared.clone();
                tokio::spawn(async move {
                    transcribe_and_publish(&ctx, &shared, wav_bytes).await;
                });
            }
            CaptureEvent::BargeIn => handle_barge_in(&ctx, &shared),
        }
    }
}

/// The user talked over a playing reply: cut the clip (and anything queued
/// behind it) so the interruption is heard, and tell the rest of the agent
/// on `agent:interrupt` — the same pair of actions agent-speech's
/// `streamVolume` takes.
///
/// A priority clip (chime, scheduler announcement) is left alone: those are
/// short, deliberate, and the Go original exempts them too.
fn handle_barge_in(ctx: &ModuleCtx, shared: &Arc<Shared>) {
    if shared.is_priority_playing.load(Ordering::SeqCst) {
        tracing::debug!("npc-speech: barge-in ignored, priority clip is playing");
        return;
    }
    shared.send_playback(PlaybackCmd::Stop);
    ctx.bus.publish(
        npc_core::topic::INTERRUPT,
        npc_core::msg::INTERRUPT,
        serde_json::json!({ "source": "barge_in" }),
    );
}

async fn transcribe_and_publish(ctx: &ModuleCtx, shared: &Arc<Shared>, wav_bytes: Vec<u8>) {
    let cfg = shared.config();
    // 接続先・モデルの解決は npc-core の resolve_llm に一本化した。
    let resolved = cfg.resolve_llm(LlmTask::Stt);
    let client = npc_llm::LlmClient::new(resolved.base_url, resolved.api_key);

    let text = match client.transcribe(&resolved.model, wav_bytes).await {
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

async fn run_chat_response_tts_task(ctx: ModuleCtx, shared: Arc<Shared>, suspend: Arc<SuspendState>) {
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

        if !shared.voice_active.load(Ordering::SeqCst) {
            tracing::debug!("npc-speech: voice loop stopped, dropping chat_response tts");
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

        let cfg = shared.config();
        let content = content.to_string();
        let shared = shared.clone();

        tokio::spawn(async move {
            let text = truncate_for_tts(&content, cfg.tts.max_len as usize);
            if let Some(wav) = synthesize_tts(&cfg, &text).await {
                if cfg.vrc.chatbox {
                    let addr = cfg.vrc.osc_address.clone();
                    let text = text.clone();
                    tokio::task::spawn_blocking(move || osc::send_chatbox(&addr, &text));
                }
                shared.send_playback(PlaybackCmd::Enqueue(wav));
            }
        });
    }
}

async fn run_interrupt_task(ctx: ModuleCtx, shared: Arc<Shared>, suspend: Arc<SuspendState>) {
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

                let shared = shared.clone();
                tokio::spawn(async move {
                    let cfg = shared.config();
                    handle_priority_tts(&cfg, content, chime_file, &shared).await;
                });
            }
            t if t == npc_core::msg::SUSPEND => {
                suspend.suspended.store(true, Ordering::SeqCst);
                let gen = suspend.generation.fetch_add(1, Ordering::SeqCst) + 1;
                shared.send_playback(PlaybackCmd::Stop);
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

/// Watches `agent:interrupt` for the web UI's voice 開始/停止 toggle and flips
/// the shared `voice_active` gate that both halves of the cascade read: the
/// capture thread stops feeding mic audio to the VAD/STT pipeline (see
/// `capture::ChunkCtx::handle_input_chunk`) and `run_chat_response_tts_task`
/// stops speaking replies.
///
/// Deliberately separate from `run_interrupt_task`, which owns the
/// *auto-expiring* suspend semantics. This gate is an explicit operator
/// switch with no timeout.
async fn run_voice_gate_task(ctx: ModuleCtx, shared: Arc<Shared>) {
    let mut rx = ctx.bus.subscribe();
    loop {
        let recv = tokio::select! {
            _ = ctx.shutdown.cancelled() => break,
            r = rx.recv() => r,
        };

        let msg = match recv {
            Ok(m) => m,
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "npc-speech: bus receiver lagged (voice gate)");
                continue;
            }
            Err(RecvError::Closed) => break,
        };

        if msg.topic != npc_core::topic::INTERRUPT {
            continue;
        }

        match msg.env.r#type.as_str() {
            t if t == npc_core::msg::VOICE_START => {
                shared.voice_active.store(true, Ordering::SeqCst);
                tracing::info!("npc-speech: voice loop started (mic listening, replies spoken)");
            }
            t if t == npc_core::msg::VOICE_STOP => {
                shared.voice_active.store(false, Ordering::SeqCst);
                // Cut whatever is already in the speaker queue too — leaving
                // the tail of a reply playing after "停止" would read as the
                // switch not having worked.
                shared.send_playback(PlaybackCmd::Stop);
                tracing::info!("npc-speech: voice loop stopped (mic muted, replies silent)");
            }
            _ => {}
        }
    }
}

async fn handle_priority_tts(cfg: &Config, content: String, chime_file: String, shared: &Arc<Shared>) {
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
        shared.send_playback(PlaybackCmd::Priority(items));
    }
}

// ---------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------

async fn synthesize_tts(cfg: &Config, text: &str) -> Option<Vec<u8>> {
    // 接続先・モデルの解決は npc-core の resolve_llm に一本化した
    // (resolve_llm(Tts) は tts.model が空なら api.model にフォールバックする
    // 従来の挙動をそのまま再現している)。voice/speed は preset の対象外なので
    // 引き続き tts セクションから直接読む。
    let resolved = cfg.resolve_llm(LlmTask::Tts);
    let client = npc_llm::LlmClient::new(resolved.base_url, resolved.api_key);

    match client
        .speak(&resolved.model, &cfg.tts.voice, text, cfg.tts.speed)
        .await
    {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn speech(input: &str, output: &str, rate: u32) -> SpeechConfig {
        SpeechConfig {
            input_device: input.to_string(),
            output_device: output.to_string(),
            input_sample_rate: rate,
            ..Default::default()
        }
    }

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

    /// An unrelated config save (a prompt edit, a scheduler tweak) must not
    /// re-open the audio devices — doing so would cut a reply off mid-word
    /// every time anything in 設定 was touched.
    #[test]
    fn unchanged_devices_do_not_restart_the_audio_threads() {
        let mut cfg = Config {
            speech: speech("mic", "spk", 16000),
            ..Default::default()
        };

        let running_speech = cfg.speech.clone();
        // `handle` is only ever joined, so a thread that returns immediately
        // stands in for a real audio thread here.
        let playback = PlaybackThread {
            tx: std_mpsc::channel().0,
            handle: std::thread::spawn(|| {}),
            speech: running_speech.clone(),
        };
        let capture = CaptureThread {
            stop: Arc::new(AtomicBool::new(false)),
            handle: std::thread::spawn(|| {}),
            speech: running_speech,
        };

        assert!(!playback_needs_restart(Some(&playback), &cfg));
        assert!(!capture_needs_restart(Some(&capture), &cfg));

        cfg.speech.output_device = "other".to_string();
        assert!(playback_needs_restart(Some(&playback), &cfg));
        assert!(!capture_needs_restart(Some(&capture), &cfg));

        cfg.speech.input_sample_rate = 48000;
        assert!(capture_needs_restart(Some(&capture), &cfg));
    }

    /// Nothing running means nothing to restart — `apply_config` starts it
    /// instead, and reporting a restart here would stop-then-start on the
    /// very first config update.
    #[test]
    fn a_stopped_thread_never_reports_needing_a_restart() {
        let cfg = Config::default();
        assert!(!playback_needs_restart(None, &cfg));
        assert!(!capture_needs_restart(None, &cfg));
    }
}
