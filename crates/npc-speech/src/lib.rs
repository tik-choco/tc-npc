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
//!
//! A `chat_response` reply is split into sentences (`split_sentences`) and
//! spoken one at a time instead of as a single TTS request: synthesizing the
//! whole reply meant total silence until all of it came back, and anything
//! past `tts.max_len` was silently thrown away rather than spoken. Each
//! sentence is also announced on `npc:ui` (`UI_MSG_TTS_LINE`) just before its
//! synthesis request goes out — this module is the only publisher of that
//! frame, which npc-server's `bus_forward` already had a `"tts_line"` arm
//! waiting for. See `run_chat_response_tts_task` / `speak_reply` for the
//! per-reply cancellation this splitting requires (a barge-in mid-reply has
//! to silence every remaining sentence, not just the one already on the
//! speaker) and the doc on `Shared::active_character` for how the voice
//! follows whichever character is active instead of always being
//! `tts.voice`.

mod capture;
mod device;
mod mixer;
mod playback;
mod resample;
mod vad;
mod wavio;

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use npc_core::config::{Config, SpeechConfig};
use npc_core::osc;
use npc_core::{Character, LlmTask, Module, ModuleCtx};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use capture::{run_capture_thread, CaptureEvent};
use playback::{run_playback_thread, PlaybackCmd, SpeakingGate};
use vad::VadParams;

/// Host audio device enumeration, re-exported for npc-server's
/// `GET /api/audio/devices` (the 音声 panel's device pickers). This is the
/// only part of the module usable without starting it — it opens no stream
/// and doesn't need the module to be enabled.
pub use device::{list_devices, AudioDevices};

/// Bus topic the web UI listens on for the mic VU meter. Not part of
/// `npc_core::bus::topic` since it's a UI-facing signal, not an
/// agent-to-agent one.
pub(crate) const UI_TOPIC: &str = "npc:ui";
const UI_MSG_VOLUME: &str = "volume";
/// "The NPC's voice is audible right now" — drives the web UI's VRM lip-sync.
/// See [`playback::SpeakingGate`] for why the browser can't work this out for
/// itself.
pub(crate) const UI_MSG_SPEAKING: &str = "speaking";
/// Continuous `0.0..=1.0` loudness of whatever TTS clip is currently
/// playing, sampled from the playback thread's actual output position (see
/// `playback::SpeakingGate::publish_level`). `UI_MSG_SPEAKING` remains the
/// authoritative on/off edge; this rides alongside it so the VRM avatar's
/// mouth can track the real envelope of the voice instead of animating a
/// fixed shape for however long a clip happens to be audible.
pub(crate) const UI_MSG_SPEAKING_LEVEL: &str = "speaking_level";
/// One sentence of a `chat_response` reply, published just before its TTS
/// synthesis request is sent (not after — the point is for the UI/extensions
/// to see the line without waiting on the round-trip). Forwarded to WS
/// clients as a `ttsLine` frame by npc-server's `bus_forward` `"tts_line"`
/// arm, which existed before this had a publisher: `bus_forward` used to
/// receive the whole (truncated) reply as one line straight off
/// `chat_response`, but now that replies are split and spoken
/// sentence-by-sentence (see the module doc), this per-sentence publish is
/// the only source of that frame.
pub(crate) const UI_MSG_TTS_LINE: &str = "tts_line";

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
    /// The same flag, wrapped so every write also announces itself on the
    /// bus for the web UI's VRM lip-sync. Playback writes go through this;
    /// the capture thread keeps reading the raw flag above.
    speaking: SpeakingGate,
    /// Narrows the above to clips barge-in must not cut (chimes, scheduler
    /// announcements).
    is_priority_playing: Arc<AtomicBool>,
    /// The web UI's 音声 開始/停止 master switch.
    voice_active: Arc<AtomicBool>,
    playback_tx: Mutex<Option<std_mpsc::Sender<PlaybackCmd>>>,
    /// Stamps the `chat_response` reply currently being spoken by
    /// `run_chat_response_tts_task`/`speak_reply`. Bumped every time a reply
    /// starts (so a newer reply always supersedes an older one still mid-way
    /// through its sentences) and by every path that must cut a reply dead —
    /// barge-in, `suspend`, disabling TTS, the voice-loop's 停止 switch — via
    /// `cancel_current_speech`.
    ///
    /// This exists because splitting a reply into sentences turned a single
    /// TTS request into several, spaced out by real synthesis latency: a
    /// `PlaybackCmd::Stop` clears whatever is already queued, but it can't
    /// stop a sentence that hasn't finished synthesizing yet from being
    /// queued *after* the Stop — that in-flight `synthesize_tts` future has
    /// no idea a barge-in happened. `speak_reply` re-checks this generation
    /// both before starting a sentence's synthesis and after it completes, so
    /// a stale sentence is dropped instead of enqueued, and the loop itself
    /// stops instead of moving on to the next sentence.
    speech_generation: AtomicU64,
    /// Cached active character (per `config.character.active_id`), so the
    /// voice it names can override `tts.voice`/the resolved TTS model
    /// (`resolve_voice`) without a disk read on every sentence spoken — that
    /// read sits inside the speech path, on the hot path of every reply.
    /// `None` means "no active character" (none set, or its file failed to
    /// load), in which case voice resolution behaves exactly as if this
    /// module had never heard of characters.
    ///
    /// Invalidation: refreshed only when a `config_updated` message reports
    /// a different `character.active_id` than last time (see the `run`
    /// loop's `last_active_id` tracking) — not on every config update, since
    /// most of those (a prompt edit, a scheduler tweak) have nothing to do
    /// with which character is speaking. Editing the *currently* active
    /// character's voice fields in place (e.g. re-importing the same tc-town
    /// export) does not carry an `active_id` change and so will not refresh
    /// this cache; nothing in the server today does that without also
    /// changing `active_id`, but it is a real edge the cache does not cover.
    active_character: RwLock<Option<Arc<Character>>>,
}

impl Shared {
    fn config(&self) -> Arc<Config> {
        self.config.read().unwrap().clone()
    }

    fn update_config(&self, config: Arc<Config>) {
        let disabled = {
            let mut current = self.config.write().unwrap();
            let disabled = current.tts.enabled && !config.tts.enabled;
            *current = config;
            disabled
        };
        if disabled {
            // Invalidate in-flight replies even if playback never opened.
            // Re-enabling TTS must only speak new replies.
            self.cancel_current_speech();
        }
    }

    fn send_playback(&self, cmd: PlaybackCmd) {
        if let Some(tx) = self.playback_tx.lock().unwrap().as_ref() {
            let _ = tx.send(cmd);
        }
    }

    /// Bump the reply generation and cut whatever is on the speaker. The
    /// single entry point for every "stop the reply dead" trigger (barge-in,
    /// `suspend`, voice-loop 停止) so all three get the same guarantee: not
    /// just silencing the clip currently playing, but also poisoning the
    /// generation stamp any of that reply's still-synthesizing sentences are
    /// checking against, so they get discarded instead of queued once they
    /// come back. See the doc on `speech_generation` for why `Stop` alone
    /// isn't enough once a reply is more than one clip.
    fn cancel_current_speech(&self) {
        self.speech_generation.fetch_add(1, Ordering::SeqCst);
        self.send_playback(PlaybackCmd::Stop);
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
        let is_tts_playing = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            config: RwLock::new(ctx.config.clone()),
            vad_params: VadParams::new(&ctx.config.stt),
            speaking: SpeakingGate::new(is_tts_playing.clone(), ctx.bus.clone()),
            is_tts_playing,
            is_priority_playing: Arc::new(AtomicBool::new(false)),
            // Starts on, so a run with no UI attached behaves exactly as it
            // did before the toggle existed.
            voice_active: Arc::new(AtomicBool::new(true)),
            playback_tx: Mutex::new(None),
            speech_generation: AtomicU64::new(0),
            // Loaded once up front so the very first reply already speaks in
            // the right voice instead of needing a config_updated to arrive
            // first.
            active_character: RwLock::new(load_active_character(&ctx.data_dir, &ctx.config)),
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

        // Tracked here rather than read back out of `shared.active_character`
        // because the id is what changes (it's what `config_updated` carries)
        // — the cached `Character` it resolves to is a derived value, not the
        // thing to compare against.
        let mut last_active_id = ctx.config.character.active_id.clone();

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
                    shared.update_config(new_config.clone());
                    shared.vad_params.set(&new_config.stt);
                    // A character can now be activated at runtime (see
                    // `api_activate_character`), and that change arrives on
                    // this same `config_updated` message — so the cached
                    // voice has to follow it here too, not just at startup.
                    // Gated on the id actually changing so an unrelated
                    // config save doesn't re-read the character file on
                    // every edit.
                    if new_config.character.active_id != last_active_id {
                        last_active_id = new_config.character.active_id.clone();
                        *shared.active_character.write().unwrap() =
                            load_active_character(&ctx.data_dir, &new_config);
                    }
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
    let playing = shared.speaking.clone();
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
    // Through the gate, not the raw flag: turning TTS off mid-sentence has to
    // tell the UI the mouth should stop moving, exactly like a clip ending.
    shared.speaking.set(false);
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
    // Not just `send_playback(Stop)`: a reply is now (potentially) several
    // sentences spaced out by real synthesis latency, so cutting the clip
    // that's audible right now isn't enough on its own — a later sentence
    // that was already mid-synthesis when the user talked over the NPC would
    // otherwise still get enqueued once it comes back, and the barge-in
    // would look like it only trimmed one sentence off a reply that then
    // kept going. See `Shared::cancel_current_speech`.
    shared.cancel_current_speech();
    ctx.bus.publish(
        npc_core::topic::INTERRUPT,
        npc_core::msg::INTERRUPT,
        serde_json::json!({ "source": "barge_in" }),
    );
}

async fn transcribe_and_publish(ctx: &ModuleCtx, shared: &Arc<Shared>, wav_bytes: Vec<u8>) {
    let cfg = shared.config();
    // 接続先・モデルの解決は npc-core の resolve_llm に一本化した。
    let resolved = match cfg.resolve_llm(LlmTask::Stt) {
        Ok(resolved) => resolved,
        Err(err) => {
            tracing::error!(error = %err, "npc-speech: stt model is unavailable");
            return;
        }
    };
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

        let cfg = shared.config();
        if !cfg.tts.enabled {
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

        let character = shared.active_character.read().unwrap().clone();
        let content = content.to_string();
        let ctx = ctx.clone();
        let shared = shared.clone();

        // Every reply gets a fresh generation, whether or not anything is
        // cancelling it: this is also what makes a second `chat_response`
        // that starts before the previous one finished speaking supersede
        // it, instead of the two replies' sentences interleaving on the
        // speaker. See `Shared::speech_generation`.
        let my_gen = shared.speech_generation.fetch_add(1, Ordering::SeqCst) + 1;

        tokio::spawn(speak_reply(ctx, shared, cfg, character, content, my_gen));
    }
}

/// Speaks one `chat_response` reply sentence by sentence, so playback can
/// start on the first sentence while the rest are still being synthesized
/// (see the module doc for why this replaced a single whole-reply TTS
/// request). `my_gen` is the stamp `speech_generation` held when this reply
/// started; re-checked before *and* after every sentence's `synthesize_tts`
/// call so a barge-in/suspend/voice-stop/newer-reply that lands while a
/// sentence is mid-synthesis drops that sentence (and every one after it)
/// instead of speaking it anyway once it comes back — see
/// `Shared::speech_generation` for why the check has to happen at both
/// points and not just once at the top of the loop.
async fn speak_reply(
    ctx: ModuleCtx,
    shared: Arc<Shared>,
    cfg: Arc<Config>,
    character: Option<Arc<Character>>,
    content: String,
    my_gen: u64,
) {
    for sentence in split_sentences(&content) {
        if !cfg.tts.enabled || !shared.config().tts.enabled {
            return;
        }
        if shared.speech_generation.load(Ordering::SeqCst) != my_gen {
            tracing::debug!("npc-speech: reply superseded/cancelled, stopping mid-reply");
            return;
        }

        // Truncated per sentence, not on the whole reply up front: a single
        // runaway sentence is still capped (protecting whatever request-size
        // limit the TTS endpoint has), but a long reply as a whole is no
        // longer thrown away past `tts.max_len` — see the module doc.
        let text = truncate_for_tts(&sentence, cfg.tts.max_len as usize);

        // Published before synthesis, not after: the point is for the
        // UI/extensions to see the line the instant it's decided, without
        // waiting on however long this sentence's TTS round-trip takes (see
        // `UI_MSG_TTS_LINE`).
        ctx.bus
            .publish(UI_TOPIC, UI_MSG_TTS_LINE, serde_json::json!({ "text": text }));

        let Some(wav) = synthesize_tts(&cfg, &text, character.as_deref()).await else {
            // Synthesis failed for this sentence: skip it (already logged in
            // `synthesize_tts`) and keep going rather than abandoning the
            // rest of the reply over one bad TTS call.
            continue;
        };

        if !shared.config().tts.enabled || shared.speech_generation.load(Ordering::SeqCst) != my_gen {
            tracing::debug!("npc-speech: reply superseded/cancelled mid-synthesis, discarding sentence");
            return;
        }

        if cfg.vrc.chatbox {
            let addr = cfg.vrc.osc_address.clone();
            let text = text.clone();
            tokio::task::spawn_blocking(move || osc::send_chatbox(&addr, &text));
        }
        shared.send_playback(PlaybackCmd::Enqueue(wav));
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
                let volume = msg
                    .env
                    .payload
                    .get("volume")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0) as f32;
                let bgm_file = msg
                    .env
                    .payload
                    .get("bgm_file")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let bgm_volume = msg
                    .env
                    .payload
                    .get("bgm_volume")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32;
                let bgm_play_full = msg
                    .env
                    .payload
                    .get("bgm_play_full")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let bgm_end_time = msg
                    .env
                    .payload
                    .get("bgm_end_time")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if content.is_empty() && chime_file.is_empty() {
                    continue;
                }

                let shared = shared.clone();
                let data_dir = ctx.data_dir.clone();
                tokio::spawn(async move {
                    let cfg = shared.config();
                    let character = shared.active_character.read().unwrap().clone();
                    handle_priority_tts(
                        &cfg,
                        &data_dir,
                        character,
                        content,
                        chime_file,
                        volume,
                        bgm_file,
                        bgm_volume,
                        bgm_play_full,
                        bgm_end_time,
                        &shared,
                    )
                    .await;
                });
            }
            t if t == npc_core::msg::SUSPEND => {
                suspend.suspended.store(true, Ordering::SeqCst);
                let gen = suspend.generation.fetch_add(1, Ordering::SeqCst) + 1;
                // Also poisons `speech_generation` (see
                // `Shared::cancel_current_speech`): without it, a reply
                // already mid-way through its later sentences' synthesis
                // when the suspend landed would still get those sentences
                // enqueued once synthesis finished, playing right through a
                // "paused" state.
                shared.cancel_current_speech();
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
            // The web UI's 割り込み button and the barge-in detector both
            // publish this. Until now it fell through the catch-all below, so
            // pressing 割り込み did nothing at all to the voice — the NPC
            // talked straight through it, which is the one thing that button
            // exists to prevent.
            //
            // One-shot, unlike `SUSPEND`: it cuts what is playing now and
            // discards the rest of this reply, but leaves the module ready to
            // speak the next one. `cancel_current_speech` is what makes
            // "the rest of this reply" stick — a reply is spoken sentence by
            // sentence, so simply stopping playback would let the sentences
            // still in synthesis arrive and resume talking a moment later.
            t if t == npc_core::msg::INTERRUPT => {
                shared.cancel_current_speech();
                tracing::info!("npc-speech: interrupted (current reply dropped)");
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
                // switch not having worked. `cancel_current_speech` (not a
                // bare `Stop`) also poisons `speech_generation`, so a
                // sentence still being synthesized when 停止 was pressed gets
                // discarded instead of being queued right after.
                shared.cancel_current_speech();
                tracing::info!("npc-speech: voice loop stopped (mic muted, replies silent)");
            }
            _ => {}
        }
    }
}

async fn handle_priority_tts(
    cfg: &Config,
    data_dir: &Path,
    character: Option<Arc<Character>>,
    content: String,
    chime_file: String,
    volume: f32,
    bgm_file: String,
    bgm_volume: f32,
    bgm_play_full: bool,
    bgm_end_time: String,
    shared: &Arc<Shared>,
) {
    if !cfg.tts.enabled || !shared.config().tts.enabled {
        return;
    }
    let mut items: Vec<Vec<u8>> = Vec::new();

    if !chime_file.is_empty() {
        // A `chime_file`/`bgm_file` is a reference into the sound library
        // (`{data_dir}/sound/`) or, for configs written before that folder
        // existed, a plain path — `resolve_sound_path` accepts both, so the
        // path logged on failure below is the one actually attempted.
        let chime_path = npc_core::resolve_sound_path(data_dir, &chime_file);
        match tokio::fs::read(&chime_path).await {
            Ok(bytes) => {
                // No BGM on the chime — `mix_announcement` is reused purely
                // for its volume-scaling path, so the chime also respects
                // the announcement's overall `volume`.
                match mix_off_thread(bytes, volume, None).await {
                    Ok(mixed) => items.push(mixed),
                    Err(err) => {
                        tracing::warn!(error = %err, file = %chime_file, path = %chime_path.display(), "npc-speech: failed to decode chime_file, skipping");
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, file = %chime_file, path = %chime_path.display(), "npc-speech: chime_file not found, skipping");
            }
        }
    }

    if !content.is_empty() {
        // Reading/mixing the chime can outlive a settings save.
        if !shared.config().tts.enabled {
            return;
        }
        // Not split into sentences: these are short, deliberate one-shot
        // announcements (scheduler chimes, `agent:interrupt` "tts"), not a
        // multi-sentence chat reply, so the time-to-first-audio problem
        // `speak_reply` exists for doesn't apply here.
        let text = truncate_for_tts(&content, cfg.tts.max_len as usize);
        if let Some(wav) = synthesize_tts(cfg, &text, character.as_deref()).await {
            if !shared.config().tts.enabled {
                return;
            }
            if cfg.vrc.chatbox {
                osc::send_chatbox(&cfg.vrc.osc_address, &text);
            }

            let bgm = if !bgm_file.is_empty() {
                let bgm_path = npc_core::resolve_sound_path(data_dir, &bgm_file);
                match tokio::fs::read(&bgm_path).await {
                    Ok(bytes) => Some(mixer::BgmMix {
                        bytes,
                        volume: bgm_volume,
                        play_full: bgm_play_full,
                        end_time: bgm_end_time.clone(),
                    }),
                    Err(err) => {
                        // Log+skip, same as chime_file above: a missing BGM
                        // file must not silence the announcement itself.
                        tracing::warn!(error = %err, file = %bgm_file, path = %bgm_path.display(), "npc-speech: bgm_file not found, playing tts without bgm");
                        None
                    }
                }
            } else {
                None
            };

            match mix_off_thread(wav, volume, bgm).await {
                Ok(mixed) => items.push(mixed),
                Err(err) => {
                    tracing::warn!(error = %err, "npc-speech: failed to mix tts announcement, skipping");
                }
            }
        }
    }

    if !items.is_empty() && shared.config().tts.enabled {
        shared.send_playback(PlaybackCmd::Priority(items));
    }
}

/// [`mixer::mix_announcement`] on a blocking thread.
///
/// It decodes, resamples and mixes whole files in one synchronous pass, and
/// a BGM track is minutes long where a TTS clip is seconds — a full-length
/// mp3 is seconds of pure CPU. Running that inline would park a runtime
/// worker for the duration, stalling every other task sharing it (the bus
/// readers, the WebSocket hub) for no reason.
async fn mix_off_thread(
    bytes: Vec<u8>,
    volume: f32,
    bgm: Option<mixer::BgmMix>,
) -> anyhow::Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || mixer::mix_announcement(&bytes, volume, bgm))
        .await
        .map_err(|err| anyhow::anyhow!("mix task failed to run: {err}"))?
}

// ---------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------

/// Terminators that end a spoken sentence: ASCII `!`/`?`, their full-width
/// Japanese equivalents, the full-width period `。`, and a bare newline (a
/// paragraph break in an LLM reply reads naturally as a pause even with no
/// punctuation before it).
///
/// The ASCII period `.` is deliberately absent, matching the reference
/// implementation's set exactly (`/(?<=[。！？!?\n])/`, tc-assistant2's
/// `splitSpeechLines`): it is overloaded in ordinary English text ("Mr.
/// Smith", "e.g.", "3.14"), so splitting on every one would chop replies
/// into nonsense far more often than it would find a real sentence boundary.
const SENTENCE_TERMINATORS: [char; 6] = ['。', '！', '？', '!', '?', '\n'];

/// Splits a reply into speakable sentences, mirroring tc-assistant2's
/// `splitSpeechLines` (`text.split(/(?<=[。！？!?\n])/).map(trim).filter(len >
/// 0)`, `src/main.tsx` ~line 134): cut immediately *after* every terminator
/// so it stays attached to the sentence it ends, trim each piece, and drop
/// anything that trims to nothing.
///
/// A run of terminators (e.g. `"!?"`, or a closing quote after a full stop
/// like `"。」"`) is deliberately *not* coalesced into one fragment — each
/// terminator ends its own piece here, exactly as in the reference
/// implementation, so `"。」"` comes back as two fragments (`"。"` and then
/// `"」"` leading whatever follows). Merging terminator runs would need
/// look-ahead the reference doesn't do either, and in practice a
/// pure-punctuation fragment just synthesizes as a fraction of a second of
/// near-silence — harmless, unlike dropping text.
///
/// Text with no terminator anywhere comes back as a single fragment: it
/// still has to be spoken, not dropped, just because the model ended the
/// reply without punctuation.
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if SENTENCE_TERMINATORS.contains(&ch) {
            sentences.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        sentences.push(current);
    }

    sentences
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Resolve which voice/model a reply should be spoken with: the active
/// character's `voice_name`/`voice_model` win when set, otherwise the global
/// `cfg_voice` (`tts.voice`) / `resolved_model` (the model `resolve_llm`
/// already picked for the TTS task). Each field falls back independently —
/// a character can name a voice but leave the model on the account default,
/// or vice versa.
///
/// A present-but-empty string (`Some("")`, as opposed to `None`) also falls
/// back rather than being sent to the API: a character imported before
/// voice fields existed, or with the field cleared in the editor, must not
/// silently downgrade to an empty `voice`/`model` parameter that the TTS
/// endpoint would either reject or silently mishandle.
fn resolve_voice(character: Option<&Character>, cfg_voice: &str, resolved_model: &str) -> (String, String) {
    let voice = character
        .and_then(|c| c.voice_name.as_deref())
        .filter(|v| !v.is_empty())
        .unwrap_or(cfg_voice)
        .to_string();
    let model = character
        .and_then(|c| c.voice_model.as_deref())
        .filter(|v| !v.is_empty())
        .unwrap_or(resolved_model)
        .to_string();
    (voice, model)
}

/// Load and cache the character named by `cfg.character.active_id`, if any
/// — see the doc on `Shared::active_character` for why this is only called
/// at startup and on an `active_id` change, never per-sentence.
fn load_active_character(data_dir: &Path, cfg: &Config) -> Option<Arc<Character>> {
    // `npc_core::active_character` already treats a missing/unparsable
    // character file as "no active character" rather than an error (see its
    // doc), so the `Err` arm here is just defense in depth against that
    // contract changing later, not a path this build expects to hit.
    npc_core::active_character(data_dir, cfg)
        .unwrap_or_else(|err| {
            tracing::warn!(error = %err, "npc-speech: failed to resolve active character, using default voice");
            None
        })
        .map(Arc::new)
}

async fn synthesize_tts(cfg: &Config, text: &str, character: Option<&Character>) -> Option<Vec<u8>> {
    // All callers, including priority announcements, must stay silent when
    // disabled: do not resolve a provider, log an error, or send a request.
    if !cfg.tts.enabled {
        return None;
    }
    // 接続先とモデルは model_ref / default_ref から解決する。
    // voice/speed は tts セクションから読み、resolve_voice が有効な
    // キャラクターの voice_name/voice_model を優先させる。
    let resolved = match cfg.resolve_llm(LlmTask::Tts) {
        Ok(resolved) => resolved,
        Err(err) => {
            tracing::error!(error = %err, "npc-speech: tts model is unavailable");
            return None;
        }
    };
    let (voice, model) = resolve_voice(character, &cfg.tts.voice, &resolved.model);
    let client = npc_llm::LlmClient::new(resolved.base_url, resolved.api_key);

    match client.speak(&model, &voice, text, cfg.tts.speed).await {
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

    #[derive(Clone, Default)]
    struct WarningCounter(Arc<AtomicU64>);

    impl tracing::Subscriber for WarningCounter {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            if *event.metadata().level() <= tracing::Level::WARN {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    fn tts_context(enabled: bool) -> (ModuleCtx, Arc<Shared>) {
        // An invalid reference fails resolution locally when enabled. No
        // network or audio device is needed to observe synthesis attempts.
        let config: Config = serde_json::from_value(serde_json::json!({
            "tts": { "enabled": enabled,
                "model_ref": { "provider_id": "missing", "model": "tts-test" } }
        }))
        .unwrap();
        let ctx = ModuleCtx {
            bus: npc_core::Bus::new(),
            config: Arc::new(config),
            config_path: Default::default(),
            shutdown: Default::default(),
            data_dir: Default::default(),
        };
        let is_tts_playing = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            config: RwLock::new(ctx.config.clone()),
            vad_params: VadParams::new(&ctx.config.stt),
            speaking: SpeakingGate::new(is_tts_playing.clone(), ctx.bus.clone()),
            is_tts_playing,
            is_priority_playing: Arc::new(AtomicBool::new(false)),
            voice_active: Arc::new(AtomicBool::new(true)),
            playback_tx: Mutex::new(None),
            speech_generation: AtomicU64::new(0),
            active_character: RwLock::new(None),
        });
        (ctx, shared)
    }

    fn set_tts_enabled(shared: &Shared, enabled: bool) {
        let mut config = (*shared.config()).clone();
        config.tts.enabled = enabled;
        shared.update_config(Arc::new(config));
    }

    #[tokio::test]
    async fn disabled_tts_skips_synthesis_and_priority_without_warnings() {
        let warnings = WarningCounter::default();
        let _guard = tracing::subscriber::set_default(warnings.clone());
        let (ctx, shared) = tts_context(false);
        let mut rx = ctx.bus.subscribe();
        assert!(synthesize_tts(&ctx.config, "hello", None).await.is_none());
        speak_reply(
            ctx.clone(),
            shared.clone(),
            ctx.config.clone(),
            None,
            "hello!".into(),
            0,
        )
        .await;
        handle_priority_tts(
            &ctx.config,
            &ctx.data_dir,
            None,
            "hello".into(),
            "missing.wav".into(),
            1.0,
            String::new(),
            0.0,
            false,
            String::new(),
            &shared,
        )
        .await;
        assert_eq!(warnings.0.load(Ordering::SeqCst), 0);
        assert!(rx.try_recv().is_err(), "disabled TTS must not announce a spoken line");

        set_tts_enabled(&shared, true);
        assert!(synthesize_tts(&shared.config(), "hello", None).await.is_none());
        assert_eq!(
            warnings.0.load(Ordering::SeqCst),
            1,
            "enabled TTS still resolves its provider"
        );
    }

    #[tokio::test]
    async fn chat_response_tts_follows_live_config_and_voice_gate() {
        let (ctx, shared) = tts_context(false);
        let mut rx = ctx.bus.subscribe();
        let suspend = Arc::new(SuspendState {
            suspended: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        });
        let task = tokio::spawn(run_chat_response_tts_task(ctx.clone(), shared.clone(), suspend));
        tokio::task::yield_now().await; // Subscribe before publishing.
        let reply = || {
            ctx.bus.publish(
                npc_core::topic::CHAT,
                npc_core::msg::CHAT_RESPONSE,
                serde_json::json!({ "content": "hello!" }),
            )
        };
        reply();
        tokio::task::yield_now().await;
        assert_eq!(shared.speech_generation.load(Ordering::SeqCst), 0);
        assert_eq!(rx.try_recv().unwrap().env.r#type, npc_core::msg::CHAT_RESPONSE);
        assert!(rx.try_recv().is_err());

        set_tts_enabled(&shared, true);
        reply();
        let line = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let message = rx.recv().await.unwrap();
                if message.env.r#type == UI_MSG_TTS_LINE {
                    break message;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(line.env.payload["text"], "hello!");
        assert_eq!(shared.speech_generation.load(Ordering::SeqCst), 1);

        set_tts_enabled(&shared, false);
        reply();
        tokio::task::yield_now().await;
        assert_eq!(shared.speech_generation.load(Ordering::SeqCst), 2);
        assert_eq!(rx.try_recv().unwrap().env.r#type, npc_core::msg::CHAT_RESPONSE);
        assert!(rx.try_recv().is_err());
        set_tts_enabled(&shared, true);
        reply();
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if rx.recv().await.unwrap().env.r#type == UI_MSG_TTS_LINE {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(shared.speech_generation.load(Ordering::SeqCst), 3);

        shared.voice_active.store(false, Ordering::SeqCst);
        reply();
        tokio::task::yield_now().await;
        assert_eq!(shared.speech_generation.load(Ordering::SeqCst), 3);
        ctx.shutdown.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn disabling_tts_cancels_old_reply_even_after_reenable() {
        let (ctx, shared) = tts_context(true);
        let mut rx = ctx.bus.subscribe();
        let (tx, playback) = std_mpsc::channel();
        *shared.playback_tx.lock().unwrap() = Some(tx);
        set_tts_enabled(&shared, false);
        assert!(matches!(playback.try_recv().unwrap(), PlaybackCmd::Stop));
        assert_eq!(shared.speech_generation.load(Ordering::SeqCst), 1);
        // The reply still holds the old enabled config snapshot.
        speak_reply(ctx.clone(), shared.clone(), ctx.config.clone(), None, "old!".into(), 0).await;
        set_tts_enabled(&shared, true);
        speak_reply(ctx.clone(), shared.clone(), ctx.config.clone(), None, "old!".into(), 0).await;
        assert!(rx.try_recv().is_err());
        assert!(playback.try_recv().is_err());
        // Unrelated/enabled saves preserve new replies' generation.
        set_tts_enabled(&shared, true);
        assert_eq!(shared.speech_generation.load(Ordering::SeqCst), 1);
    }

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

    // -------------------------------------------------------------
    // split_sentences
    // -------------------------------------------------------------

    #[test]
    fn splits_on_full_width_japanese_terminators() {
        assert_eq!(
            split_sentences("こんにちは。元気ですか？うん！"),
            vec!["こんにちは。", "元気ですか？", "うん！"]
        );
    }

    #[test]
    fn splits_on_ascii_terminators() {
        assert_eq!(
            split_sentences("How are you? I'm great! Really?"),
            vec!["How are you?", "I'm great!", "Really?"]
        );
    }

    /// The reference splitter's terminator set (`。！？!?\n`) deliberately
    /// excludes the ASCII period: unlike `!`/`?`/full-width punctuation, a
    /// bare `.` is overloaded in ordinary English text ("Mr. Smith", "e.g.",
    /// "3.14") and splitting on every one of those would chop a reply into
    /// nonsense far more often than it would find a real sentence boundary.
    /// So a period-only reply comes back as a single, unsplit fragment.
    #[test]
    fn ascii_period_does_not_split_a_sentence() {
        assert_eq!(
            split_sentences("Hello there. How are you today."),
            vec!["Hello there. How are you today."]
        );
    }

    #[test]
    fn splits_on_bare_newlines_even_without_punctuation() {
        assert_eq!(
            split_sentences("line one\nline two\nline three"),
            vec!["line one", "line two", "line three"]
        );
    }

    /// No terminator anywhere: the whole reply must still come back as one
    /// piece to be spoken, never dropped for lacking punctuation.
    #[test]
    fn untermined_text_is_spoken_as_one_piece() {
        assert_eq!(
            split_sentences("no terminator anywhere in this reply"),
            vec!["no terminator anywhere in this reply"]
        );
    }

    #[test]
    fn empty_and_whitespace_only_input_yields_no_sentences() {
        assert!(split_sentences("").is_empty());
        assert!(split_sentences("   \n\t  ").is_empty());
    }

    /// A run of terminators is not coalesced: each one ends its own
    /// fragment, matching tc-assistant2's reference splitter exactly (see
    /// `split_sentences`'s doc). `"!?"` therefore comes back as two pieces.
    #[test]
    fn a_terminator_run_splits_into_multiple_fragments() {
        assert_eq!(split_sentences("えっ!?"), vec!["えっ!", "?"]);
    }

    /// Same shape as above but with a full-width period followed by a
    /// closing quote mark that is not itself a terminator: the quote leads
    /// its own (otherwise empty) fragment rather than staying attached to
    /// the sentence it visually closes.
    #[test]
    fn a_closing_quote_after_a_terminator_becomes_its_own_fragment() {
        assert_eq!(split_sentences("そう言った。」"), vec!["そう言った。", "」"]);
    }

    /// Blank lines between sentences (a common LLM formatting habit) must
    /// not produce empty spoken fragments.
    #[test]
    fn blank_lines_between_sentences_are_dropped() {
        assert_eq!(
            split_sentences("最初の文。\n\n次の文。"),
            vec!["最初の文。", "次の文。"]
        );
    }

    /// A very long sentence with no internal terminator must survive whole
    /// (truncation, if any, is a separate concern applied by
    /// `truncate_for_tts` at the call site — see `speak_reply`).
    #[test]
    fn a_very_long_untermined_sentence_is_not_split_or_dropped() {
        let long = "a".repeat(5000);
        let sentences = split_sentences(&long);
        assert_eq!(sentences.len(), 1);
        assert_eq!(sentences[0].chars().count(), 5000);
    }

    // -------------------------------------------------------------
    // resolve_voice
    // -------------------------------------------------------------

    /// Builds a `Character` with only the voice fields populated (the rest
    /// of the sheet is irrelevant to voice resolution).
    fn character_with_voice(voice_name: Option<&str>, voice_model: Option<&str>) -> Character {
        Character {
            id: "c1".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
            sheet: npc_core::CharacterSheet::default(),
            voice_model: voice_model.map(|s| s.to_string()),
            voice_name: voice_name.map(|s| s.to_string()),
            avatar: None,
        }
    }

    #[test]
    fn no_active_character_falls_back_to_global_config() {
        let (voice, model) = resolve_voice(None, "alloy", "tts-1");
        assert_eq!(voice, "alloy");
        assert_eq!(model, "tts-1");
    }

    #[test]
    fn active_character_voice_overrides_global_config() {
        let c = character_with_voice(Some("shimmer"), Some("tts-1-hd"));
        let (voice, model) = resolve_voice(Some(&c), "alloy", "tts-1");
        assert_eq!(voice, "shimmer");
        assert_eq!(model, "tts-1-hd");
    }

    /// Each field falls back independently — a character can pin a voice
    /// without also pinning a model, or vice versa.
    #[test]
    fn voice_and_model_fall_back_independently() {
        let voice_only = character_with_voice(Some("shimmer"), None);
        let (voice, model) = resolve_voice(Some(&voice_only), "alloy", "tts-1");
        assert_eq!(voice, "shimmer");
        assert_eq!(model, "tts-1");

        let model_only = character_with_voice(None, Some("tts-1-hd"));
        let (voice, model) = resolve_voice(Some(&model_only), "alloy", "tts-1");
        assert_eq!(voice, "alloy");
        assert_eq!(model, "tts-1-hd");
    }

    /// A present-but-empty field (imported before voice fields existed, or
    /// cleared in the editor) must fall back exactly like `None` — never
    /// forwarded to the API as an empty string.
    #[test]
    fn empty_but_present_voice_fields_fall_back_too() {
        let c = character_with_voice(Some(""), Some(""));
        let (voice, model) = resolve_voice(Some(&c), "alloy", "tts-1");
        assert_eq!(voice, "alloy");
        assert_eq!(model, "tts-1");
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
