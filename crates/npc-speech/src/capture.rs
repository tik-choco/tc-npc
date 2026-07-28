//! Mic capture thread: owns the cpal input stream (cpal streams are `!Send`,
//! so this lives on its own dedicated OS thread, never touching tokio),
//! resamples/downmixes to mono `config.speech.input_sample_rate`, runs it
//! through [`crate::vad::VadState`], and reports events back to the async
//! world over an unbounded tokio channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, StreamTrait};
use npc_core::config::SpeechConfig;
use tokio::sync::mpsc::UnboundedSender;

use crate::device::select_input_device;
use crate::resample::{downmix_to_mono, resample_linear, rms_f32, rms_i16};
use crate::vad::{VadParams, VadState};
use crate::wavio::encode_wav_i16;

/// How often (at most) mic-level events are published to the bus, per the
/// port spec ("~10x/sec").
const VOLUME_PUBLISH_INTERVAL: Duration = Duration::from_millis(100);
/// Multiplier applied to raw RMS before clamping to `[0, 1]` for the UI
/// meter; raw speech RMS on a normalized `f32` signal is usually well under
/// 1.0, so this keeps the meter from reading as "always near zero".
const VOLUME_UI_GAIN: f64 = 4.0;

pub enum CaptureEvent {
    /// Mic level for the UI VU meter, already scaled/clamped to `[0, 1]`.
    Volume(f64),
    /// A finished, worth-transcribing speech segment, encoded as a 16-bit
    /// PCM mono WAV at `config.speech.input_sample_rate`.
    Segment(Vec<u8>),
    /// The user started talking over a playing TTS clip (see
    /// [`BargeInDetector`]). The async side stops playback so the
    /// interruption lands, mirroring agent-speech's `streamVolume`.
    BargeIn,
}

/// Tracks sustained above-barge-in-threshold mic audio while a TTS clip is
/// playing. Sustain rather than a single loud buffer, because one cough or a
/// chair scrape at speech level would otherwise cut the agent off mid-word;
/// the window is the same [`crate::vad::START_VOICE_MS`] the VAD uses to
/// decide a segment has really begun.
struct BargeInDetector {
    /// Above-threshold samples seen back-to-back, reset by any quiet buffer.
    loud_samples: usize,
    limit: usize,
    /// Set once a barge-in has been reported for the clip currently playing,
    /// so one interruption sends one `Stop` rather than one per buffer while
    /// playback winds down.
    fired: bool,
}

impl BargeInDetector {
    fn new(sample_rate: u32) -> Self {
        Self {
            loud_samples: 0,
            limit: crate::vad::ms_to_samples(sample_rate, crate::vad::START_VOICE_MS),
            fired: false,
        }
    }

    /// Feed one buffer captured while TTS is playing. Returns true exactly
    /// once per clip, on the buffer that completes the sustain window.
    fn observe(&mut self, chunk: &[i16], threshold: f32) -> bool {
        if self.fired {
            return false;
        }
        let loud = threshold <= 0.0 || rms_i16(chunk) > threshold;
        if !loud {
            self.loud_samples = 0;
            return false;
        }
        self.loud_samples += chunk.len();
        if self.loud_samples >= self.limit {
            self.fired = true;
            return true;
        }
        false
    }

    /// Called on every buffer captured while nothing is playing — the clip
    /// that was interrupted (or simply finished) is over, so the next one
    /// gets a fresh window.
    fn rearm(&mut self) {
        self.loud_samples = 0;
        self.fired = false;
    }
}

/// Entry point run on a dedicated `std::thread`. Logs and returns on setup
/// failure (e.g. no input device) rather than panicking the thread.
/// `voice_active` is the web UI's 音声 開始/停止 gate (see
/// `crate::run_voice_gate_task`). It only silences the *pipeline* — the cpal
/// stream keeps running so that flipping the switch back on resumes
/// instantly rather than re-opening the device.
///
/// `vad_params` is shared with the module's config loop, which rewrites it
/// on every config update, so threshold/silence/barge-in changes are heard on
/// the next buffer. The device and sample rate in `speech` are fixed for the
/// life of the thread instead — changing either means re-opening the stream,
/// which `crate::apply_config` does by stopping this thread and starting a
/// new one.
pub fn run_capture_thread(
    speech: SpeechConfig,
    vad_params: Arc<VadParams>,
    events_tx: UnboundedSender<CaptureEvent>,
    stop: Arc<AtomicBool>,
    tts_playing: Arc<AtomicBool>,
    voice_active: Arc<AtomicBool>,
) {
    if let Err(err) = run_capture_inner(speech, vad_params, events_tx, stop, tts_playing, voice_active) {
        tracing::error!(error = %err, "npc-speech: capture thread failed");
    }
}

fn run_capture_inner(
    speech: SpeechConfig,
    vad_params: Arc<VadParams>,
    events_tx: UnboundedSender<CaptureEvent>,
    stop: Arc<AtomicBool>,
    tts_playing: Arc<AtomicBool>,
    voice_active: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let host = cpal::default_host();
    let device = select_input_device(&host, &speech.input_device)?;
    tracing::info!(
        device = device.name().unwrap_or_default(),
        "npc-speech: capturing mic input"
    );

    let device_cfg = device.default_input_config()?;
    let channels = device_cfg.channels();
    let device_rate = device_cfg.sample_rate().0;
    let sample_format = device_cfg.sample_format();
    let target_rate = speech.input_sample_rate.max(1000);
    let stream_config: cpal::StreamConfig = device_cfg.into();

    let vad = Arc::new(Mutex::new(VadState::new(target_rate, vad_params.clone())));
    let barge_in = Arc::new(Mutex::new(BargeInDetector::new(target_rate)));
    let last_volume_publish = Arc::new(Mutex::new(
        Instant::now()
            .checked_sub(VOLUME_PUBLISH_INTERVAL)
            .unwrap_or_else(Instant::now),
    ));

    let err_fn = |err| tracing::error!(error = %err, "npc-speech: input stream error");

    // Everything the cpal callback needs, bundled so both sample-format
    // branches capture one `Arc` instead of eight.
    let chunk_ctx = Arc::new(ChunkCtx {
        device_rate,
        target_rate,
        vad_params,
        vad,
        barge_in,
        events_tx: events_tx.clone(),
        tts_playing,
        voice_active,
        last_volume_publish,
    });

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let ctx = chunk_ctx.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    ctx.handle_input_chunk(downmix_to_mono(data, channels));
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let ctx = chunk_ctx.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let f32_data: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                    ctx.handle_input_chunk(downmix_to_mono(&f32_data, channels));
                },
                err_fn,
                None,
            )?
        }
        other => anyhow::bail!("unsupported input sample format: {other:?}"),
    };

    stream.play()?;

    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(stream);

    // Flush any in-progress segment on shutdown, matching the Go original's
    // post-loop flush.
    if let Some(pcm) = chunk_ctx.vad.lock().unwrap().flush() {
        if let Ok(wav) = encode_wav_i16(&pcm, target_rate) {
            let _ = events_tx.send(CaptureEvent::Segment(wav));
        }
    }

    Ok(())
}

/// The shared state one cpal input callback touches per buffer.
struct ChunkCtx {
    device_rate: u32,
    target_rate: u32,
    vad_params: Arc<VadParams>,
    vad: Arc<Mutex<VadState>>,
    barge_in: Arc<Mutex<BargeInDetector>>,
    events_tx: UnboundedSender<CaptureEvent>,
    tts_playing: Arc<AtomicBool>,
    voice_active: Arc<AtomicBool>,
    last_volume_publish: Arc<Mutex<Instant>>,
}

impl ChunkCtx {
    fn handle_input_chunk(&self, mono_device_rate: Vec<f32>) {
        let resampled = if self.device_rate == self.target_rate {
            mono_device_rate
        } else {
            resample_linear(&mono_device_rate, self.device_rate, self.target_rate)
        };
        if resampled.is_empty() {
            return;
        }

        let listening = self.voice_active.load(Ordering::Relaxed);

        // Mic level, throttled to roughly VOLUME_PUBLISH_INTERVAL. While the
        // voice loop is stopped we keep publishing, but as a flat zero — the UI
        // meter has to settle at silence rather than freeze on the last level it
        // saw before the switch was flipped.
        {
            let mut last = self.last_volume_publish.lock().unwrap();
            if last.elapsed() >= VOLUME_PUBLISH_INTERVAL {
                *last = Instant::now();
                let level = if listening {
                    ((rms_f32(&resampled) as f64) * VOLUME_UI_GAIN).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let _ = self.events_tx.send(CaptureEvent::Volume(level));
            }
        }

        // Voice loop stopped from the UI: the stream stays open (so restarting is
        // instant) but nothing reaches the VAD/STT pipeline.
        if !listening {
            self.vad.lock().unwrap().reset();
            self.barge_in.lock().unwrap().rearm();
            return;
        }

        let pcm_i16: Vec<i16> = resampled
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect();

        if self.tts_playing.load(Ordering::Relaxed) {
            self.handle_chunk_during_playback(&pcm_i16);
            return;
        }

        self.barge_in.lock().unwrap().rearm();
        let segment = { self.vad.lock().unwrap().process(&pcm_i16) };
        if let Some(pcm) = segment {
            match encode_wav_i16(&pcm, self.target_rate) {
                Ok(wav) => {
                    let _ = self.events_tx.send(CaptureEvent::Segment(wav));
                }
                    Err(err) => tracing::error!(error = %err, "npc-speech: failed to encode wav segment"),
            }
        }
    }

    /// Mic audio captured while a TTS clip is playing.
    ///
    /// With `stt.barge_in` off this is pure echo prevention: the buffer is
    /// dropped and the VAD reset, so the agent's own voice can't be
    /// transcribed as if the user had said it.
    ///
    /// With it on the buffer still doesn't reach the VAD — that would let the
    /// reply talk to itself — but it is measured against the raised
    /// barge-in threshold, and sustained speech above it emits
    /// [`CaptureEvent::BargeIn`]. `lib.rs` stops playback in response, which
    /// clears `tts_playing` within a buffer or two, and from there the
    /// interruption is captured by the ordinary VAD path above.
    fn handle_chunk_during_playback(&self, pcm_i16: &[i16]) {
        self.vad.lock().unwrap().reset();

        if !self.vad_params.barge_in_enabled() {
            return;
        }

        let threshold = self.vad_params.barge_in_threshold();
        let triggered = { self.barge_in.lock().unwrap().observe(pcm_i16, threshold) };
        if triggered {
            tracing::info!("npc-speech: barge-in detected, stopping playback");
            let _ = self.events_tx.send(CaptureEvent::BargeIn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 16kHz => 120ms of sustain is 1920 samples.
    const LIMIT_SAMPLES: usize = 1920;

    fn loud(n: usize) -> Vec<i16> {
        vec![20000; n]
    }
    fn quiet(n: usize) -> Vec<i16> {
        vec![0; n]
    }

    #[test]
    fn barge_in_needs_sustained_audio_not_a_single_loud_buffer() {
        let mut detector = BargeInDetector::new(16000);
        // A blip well under the sustain window, then silence, must not fire.
        assert!(!detector.observe(&loud(LIMIT_SAMPLES / 4), 0.1));
        assert!(!detector.observe(&quiet(160), 0.1));
        // The blip's credit is gone, so an identical one still doesn't fire.
        assert!(!detector.observe(&loud(LIMIT_SAMPLES / 4), 0.1));
    }

    #[test]
    fn barge_in_fires_once_per_clip_then_rearms() {
        let mut detector = BargeInDetector::new(16000);
        assert!(!detector.observe(&loud(LIMIT_SAMPLES / 2), 0.1));
        assert!(detector.observe(&loud(LIMIT_SAMPLES / 2), 0.1));
        // Playback is winding down but still flagged: no second Stop.
        assert!(!detector.observe(&loud(LIMIT_SAMPLES), 0.1));

        // Playback ended, so the next clip gets a fresh window.
        detector.rearm();
        assert!(detector.observe(&loud(LIMIT_SAMPLES), 0.1));
    }

    /// `loud_chunk` sits at RMS ~0.61 of full scale. A barge-in threshold
    /// above that must keep the agent's own voice from interrupting it.
    #[test]
    fn audio_below_the_barge_in_threshold_never_fires() {
        let mut detector = BargeInDetector::new(16000);
        assert!(!detector.observe(&loud(LIMIT_SAMPLES * 4), 0.9));
    }
}
