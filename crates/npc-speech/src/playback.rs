//! TTS playback thread: owns the cpal output stream (also `!Send`, also
//! lives on its own dedicated OS thread). Consumes [`PlaybackCmd`]s from a
//! `std::sync::mpsc` channel fed by the async bus-subscriber tasks in
//! `lib.rs`, queueing normal `chat_response` clips and letting
//! `agent:interrupt` "tts" messages jump the queue.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, StreamTrait};
use npc_core::config::SpeechConfig;
use npc_core::Bus;

use crate::device::select_output_device;
use crate::resample::{resample_linear, rms_f32};
use crate::wavio::decode_wav;
use crate::{UI_MSG_SPEAKING, UI_MSG_SPEAKING_LEVEL, UI_TOPIC};

/// Extra silence appended after the last sample is handed to the device, so
/// we don't cut the tail off before the device's internal buffer has
/// actually played it.
const TAIL_LATENCY: Duration = Duration::from_millis(120);
const POLL_INTERVAL: Duration = Duration::from_millis(15);

/// How often (at most) the current playback loudness is forwarded to the
/// bus for the web UI's lip-sync. Recomputing the RMS window is cheap (each
/// one is only a `POLL_INTERVAL` slice of samples), but the browser can't
/// visibly use updates any faster than this — it eases between the levels
/// it receives anyway — so publishing every tick would just be extra
/// bus/WS traffic.
const LEVEL_PUBLISH_INTERVAL: Duration = Duration::from_millis(50);

/// Gain applied to a window's raw RMS before clamping to `[0, 1]` for the
/// lip-sync level. Synthesized speech sits well under full scale on a
/// normalized signal (the same reason `capture::VOLUME_UI_GAIN` exists for
/// the mic meter), so a bare RMS reading would barely crack the mouth open.
/// This scales an ordinary speaking voice up toward fully open; the clamp
/// keeps a loud clip (or a slightly generous gain) from overdriving the
/// browser's animation.
const PLAYBACK_LEVEL_GAIN: f64 = 6.0;

/// Maps one window's RMS to a `0.0..=1.0` mouth-openness level. Pulled out
/// as a pure function so the gain/clamp choice can be unit-tested without
/// standing up a cpal stream.
fn rms_to_level(rms: f32) -> f64 {
    (rms as f64 * PLAYBACK_LEVEL_GAIN).clamp(0.0, 1.0)
}

pub enum PlaybackCmd {
    /// A normal `agent:chat` `chat_response` clip: queued after whatever is
    /// currently playing/queued.
    Enqueue(Vec<u8>),
    /// An `agent:interrupt` "tts" clip (optionally preceded by a chime):
    /// clears the queue and interrupts whatever is currently playing.
    Priority(Vec<Vec<u8>>),
    /// An `agent:interrupt` "suspend": stop and clear the queue, but keep
    /// the thread alive.
    Stop,
    Shutdown,
}

enum PlayOutcome {
    Done,
    Shutdown,
}

/// One queued clip plus whether it arrived as a priority (`agent:interrupt`
/// "tts") item. Scheduler announcements and chimes are priority, and
/// barge-in deliberately does not cut those short — same carve-out as the Go
/// original's `!a.isPriorityPlaying.Load()`.
type QueuedClip = (Vec<u8>, bool);

/// The "the NPC's voice is audible right now" flag, plus the bus it is
/// announced on.
///
/// The flag itself is the mic's echo/barge-in gate, read by the capture
/// thread. The announcement exists for the web UI's VRM avatar: the browser
/// never receives the audio (TTS is synthesized and played entirely on the
/// host), so the only way its mouth can move in time with the voice is for
/// the process actually driving the speaker to say when it starts and stops.
/// The `ttsLine` frame can't stand in for that — it is sent when the *text*
/// is ready, which is before synthesis has even been requested.
///
/// Publishes only on a real transition, so holding the flag steady through a
/// multi-clip reply doesn't spam a frame per clip.
#[derive(Clone)]
pub struct SpeakingGate {
    flag: Arc<AtomicBool>,
    bus: Bus,
}

impl SpeakingGate {
    pub fn new(flag: Arc<AtomicBool>, bus: Bus) -> Self {
        SpeakingGate { flag, bus }
    }

    /// Set the flag, announcing the change if this actually flipped it.
    pub fn set(&self, active: bool) {
        if self.flag.swap(active, Ordering::SeqCst) == active {
            return;
        }
        self.bus
            .publish(UI_TOPIC, UI_MSG_SPEAKING, serde_json::json!({ "active": active }));
        if !active {
            // The mouth has to close even if the last loudness tick
            // published was loud: barge-in, `stop_playback`, and the
            // voice-loop's 停止 switch can all cut a clip off mid-word,
            // well before the level would have decayed to silence on its
            // own. Publishing on every inactive transition (rather than
            // only at the natural end of `play_wav_blocking`) covers all of
            // those cases from one place.
            self.publish_level(0.0);
        }
    }

    /// Publish the current playback loudness for the web UI's lip-sync (see
    /// module doc). Unlike `set`, this is not gated on a transition — it is
    /// the continuous signal the browser animates the mouth from while a
    /// clip is playing.
    pub fn publish_level(&self, level: f64) {
        self.bus.publish(
            UI_TOPIC,
            UI_MSG_SPEAKING_LEVEL,
            serde_json::json!({ "level": level }),
        );
    }
}

/// Entry point run on a dedicated `std::thread`. `is_playing` is the mic's
/// echo/barge-in gate (and the web UI's lip-sync signal, see
/// [`SpeakingGate`]); `is_priority_playing` narrows it to "and the clip is
/// one the user isn't allowed to talk over".
pub fn run_playback_thread(
    speech: SpeechConfig,
    rx: Receiver<PlaybackCmd>,
    is_playing: SpeakingGate,
    is_priority_playing: Arc<AtomicBool>,
) {
    let host = cpal::default_host();
    let device = match select_output_device(&host, &speech.output_device) {
        Ok(d) => d,
        Err(err) => {
            tracing::error!(error = %err, "npc-speech: no output audio device available, TTS playback disabled");
            return;
        }
    };
    tracing::info!(
        device = device.name().unwrap_or_default(),
        "npc-speech: TTS playback device ready"
    );

    let mut queue: VecDeque<QueuedClip> = VecDeque::new();

    'outer: loop {
        if let Some((wav, priority)) = queue.pop_front() {
            is_priority_playing.store(priority, Ordering::SeqCst);
            let outcome = play_wav_blocking(&device, &wav, &rx, &mut queue, &is_playing);
            is_priority_playing.store(false, Ordering::SeqCst);
            match outcome {
                PlayOutcome::Shutdown => break 'outer,
                PlayOutcome::Done => {}
            }
            continue;
        }

        match rx.recv() {
            Ok(PlaybackCmd::Enqueue(wav)) => queue.push_back((wav, false)),
            Ok(PlaybackCmd::Priority(items)) => {
                queue.clear();
                queue.extend(items.into_iter().map(|wav| (wav, true)));
            }
            Ok(PlaybackCmd::Stop) => queue.clear(),
            Ok(PlaybackCmd::Shutdown) | Err(_) => break 'outer,
        }
    }
}

/// Decode+play one clip, polling `rx` for interrupting commands while it
/// plays. Enqueue commands received mid-playback are appended to `queue`
/// without interrupting; Priority/Stop/Shutdown interrupt immediately.
fn play_wav_blocking(
    device: &cpal::Device,
    wav: &[u8],
    rx: &Receiver<PlaybackCmd>,
    queue: &mut VecDeque<QueuedClip>,
    is_playing: &SpeakingGate,
) -> PlayOutcome {
    let (samples, src_rate, src_channels) = match decode_wav(wav) {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(error = %err, "npc-speech: failed to decode tts wav");
            return PlayOutcome::Done;
        }
    };
    let mono: Vec<f32> = if src_channels <= 1 {
        samples
    } else {
        samples
            .chunks(src_channels as usize)
            .map(|frame| frame.iter().sum::<f32>() / src_channels as f32)
            .collect()
    };

    let out_cfg = match device.default_output_config() {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(error = %err, "npc-speech: failed to read output device config");
            return PlayOutcome::Done;
        }
    };
    let device_rate = out_cfg.sample_rate().0;
    let channels = out_cfg.channels();
    let sample_format = out_cfg.sample_format();
    let stream_config: cpal::StreamConfig = out_cfg.into();

    let resampled = Arc::new(resample_linear(&mono, src_rate, device_rate));
    let total = resampled.len();
    if total == 0 {
        return PlayOutcome::Done;
    }

    let position = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicBool::new(false));
    let err_fn = |err| tracing::error!(error = %err, "npc-speech: output stream error");

    let stream_result = match sample_format {
        cpal::SampleFormat::F32 => {
            let samples = resampled.clone();
            let position = position.clone();
            let finished = finished.clone();
            device.build_output_stream(
                &stream_config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    fill_output_f32(out, channels, &samples, &position, &finished, total);
                },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let samples = resampled.clone();
            let position = position.clone();
            let finished = finished.clone();
            device.build_output_stream(
                &stream_config,
                move |out: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    fill_output_i16(out, channels, &samples, &position, &finished, total);
                },
                err_fn,
                None,
            )
        }
        other => {
            tracing::error!("npc-speech: unsupported output sample format: {other:?}");
            return PlayOutcome::Done;
        }
    };

    let stream = match stream_result {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(error = %err, "npc-speech: failed to build output stream");
            return PlayOutcome::Done;
        }
    };
    if let Err(err) = stream.play() {
        tracing::error!(error = %err, "npc-speech: failed to start output stream");
        return PlayOutcome::Done;
    }

    is_playing.set(true);
    let mut outcome = PlayOutcome::Done;

    // `level_pos` is the position last folded into a loudness window, kept
    // separate from `last_level_publish` (when a level was last actually
    // sent) because the two are throttled independently: we still want a
    // fresh, short window every tick rather than one that grows across
    // however many ticks a publish gets skipped for.
    let mut level_pos = 0usize;
    let mut last_level_publish = Instant::now()
        .checked_sub(LEVEL_PUBLISH_INTERVAL)
        .unwrap_or_else(Instant::now);

    loop {
        if finished.load(Ordering::SeqCst) {
            break;
        }

        // Loudness for the web UI's lip-sync: fold in whatever the audio
        // callback has consumed since the last tick. Done here, not in
        // fill_output_f32/fill_output_i16 — those run on cpal's realtime
        // callback, which must not gain work or allocate, whereas this
        // thread is just polling `rx` anyway. No attack/decay smoothing is
        // applied: the browser already eases between the levels it
        // receives, so smoothing here too would just double-filter the
        // same transition and add latency for no visible benefit.
        let pos = position.load(Ordering::SeqCst).min(total);
        if pos > level_pos {
            if last_level_publish.elapsed() >= LEVEL_PUBLISH_INTERVAL {
                let level = rms_to_level(rms_f32(&resampled[level_pos..pos]));
                is_playing.publish_level(level);
                last_level_publish = Instant::now();
            }
            level_pos = pos;
        }

        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(PlaybackCmd::Enqueue(w)) => queue.push_back((w, false)),
            Ok(PlaybackCmd::Priority(items)) => {
                queue.clear();
                queue.extend(items.into_iter().map(|wav| (wav, true)));
                break;
            }
            Ok(PlaybackCmd::Stop) => {
                queue.clear();
                break;
            }
            Ok(PlaybackCmd::Shutdown) => {
                queue.clear();
                outcome = PlayOutcome::Shutdown;
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                outcome = PlayOutcome::Shutdown;
                break;
            }
        }
    }

    if finished.load(Ordering::SeqCst) {
        std::thread::sleep(TAIL_LATENCY);
    }
    drop(stream);
    is_playing.set(false);

    // Drain any commands that arrived in the same instant playback ended,
    // without blocking further.
    loop {
        match rx.try_recv() {
            Ok(PlaybackCmd::Enqueue(w)) => queue.push_back((w, false)),
            Ok(PlaybackCmd::Priority(items)) => {
                queue.clear();
                queue.extend(items.into_iter().map(|wav| (wav, true)));
            }
            Ok(PlaybackCmd::Stop) => queue.clear(),
            Ok(PlaybackCmd::Shutdown) => {
                queue.clear();
                outcome = PlayOutcome::Shutdown;
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                outcome = PlayOutcome::Shutdown;
                break;
            }
        }
    }

    outcome
}

fn fill_output_f32(
    out: &mut [f32],
    channels: u16,
    samples: &Arc<Vec<f32>>,
    position: &Arc<AtomicUsize>,
    finished: &Arc<AtomicBool>,
    total: usize,
) {
    let ch = channels.max(1) as usize;
    for frame in out.chunks_mut(ch) {
        let p = position.fetch_add(1, Ordering::SeqCst);
        let s = if p < total { samples[p] } else { 0.0 };
        for slot in frame.iter_mut() {
            *slot = s;
        }
        if p + 1 >= total {
            finished.store(true, Ordering::SeqCst);
        }
    }
}

fn fill_output_i16(
    out: &mut [i16],
    channels: u16,
    samples: &Arc<Vec<f32>>,
    position: &Arc<AtomicUsize>,
    finished: &Arc<AtomicBool>,
    total: usize,
) {
    let ch = channels.max(1) as usize;
    for frame in out.chunks_mut(ch) {
        let p = position.fetch_add(1, Ordering::SeqCst);
        let s = if p < total {
            (samples[p].clamp(-1.0, 1.0) * 32767.0) as i16
        } else {
            0
        };
        for slot in frame.iter_mut() {
            *slot = s;
        }
        if p + 1 >= total {
            finished.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::rms_to_level;

    #[test]
    fn silence_maps_to_zero() {
        assert_eq!(rms_to_level(0.0), 0.0);
    }

    /// Ordinary speech RMS on a normalized signal is on the order of a few
    /// percent of full scale; the gain has to bring that up near the top of
    /// the range rather than leaving the mouth barely open.
    #[test]
    fn typical_speech_rms_reads_as_mostly_open() {
        let level = rms_to_level(0.12);
        assert!(level > 0.5, "expected a well-open mouth, got {level}");
    }

    /// A loud clip (or a slightly generous gain) must never exceed 1.0 —
    /// the browser has nothing sensible to do with an out-of-range level.
    #[test]
    fn loud_rms_clamps_to_one() {
        assert_eq!(rms_to_level(1.0), 1.0);
        assert_eq!(rms_to_level(5.0), 1.0);
    }
}
