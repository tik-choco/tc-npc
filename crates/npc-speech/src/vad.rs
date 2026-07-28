//! RMS-based voice-activity segmentation, ported from the OpenAI-STT path of
//! Go `agent-speech` (`streamOpenAISTTAudio` in `internal/app/app.go`).
//!
//! Operates on mono 16-bit PCM chunks already resampled to the target rate
//! (see `resample.rs`). Each `process()` call corresponds to one audio
//! capture buffer (analogous to one `<-cap.DataChan()` receive in the Go
//! original).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Pre-roll ring buffer kept before speech is detected, so the segment sent
/// to STT includes a little audio from just before the threshold was
/// crossed (Go: `STTPreRollDuration`).
const PRE_ROLL_MS: u64 = 300;
/// Consecutive above-threshold audio required before a segment is
/// considered "started" (Go: `STTStartVoiceDuration`). Also reused by
/// `capture.rs` as the sustain a barge-in has to clear.
pub(crate) const START_VOICE_MS: u64 = 120;
/// Minimum total voiced duration for a finished segment to be worth
/// transcribing (Go: `STTMinVoicedDuration`).
const MIN_VOICED_MS: u64 = 250;
/// A finished segment's overall RMS must be at least `threshold *
/// MIN_RMS_FACTOR` to be transcribed (Go: `STTMinRMSFactor`).
const MIN_RMS_FACTOR: f32 = 0.25;
/// Hard cap on a single segment's length, so a stuck-open mic can't buffer
/// forever (requested explicitly by the tc-npc port spec; the Go original
/// has no equivalent constant in this excerpt).
const MAX_SEGMENT_SECS: f32 = 30.0;
/// Fallback silence delay used if `config.stt.silence_duration` is <= 0.
const DEFAULT_SILENCE_SECS: f32 = 1.5;

/// The two VAD knobs the UI can move mid-conversation
/// (`config.stt.input_threshold` and `config.stt.silence_duration`), held
/// apart from [`VadState`] so they can be written from outside the capture
/// thread.
///
/// Atomics rather than a `Mutex`: the reader is the cpal input callback,
/// which runs on a realtime audio thread and must never block waiting on a
/// lock the tokio side happens to hold. Both are read once per audio buffer,
/// so a change is heard on the next chunk — see `run_vad_config_task` in
/// lib.rs for the writer.
///
/// `threshold` is stored exactly as configured: RMS of the normalized f32
/// signal, compared against a chunk's raw RMS. It is *not* on the UI meter's
/// scale, which is gain-scaled for legibility (capture.rs's
/// `VOLUME_UI_GAIN`).
#[derive(Debug)]
pub struct VadParams {
    /// `f32::to_bits` — there is no atomic float.
    threshold_bits: AtomicU32,
    /// Already resolved: a non-positive configured value is stored as
    /// [`DEFAULT_SILENCE_SECS`], so readers never re-apply that fallback.
    silence_ms: AtomicU64,
    /// `config.stt.barge_in`.
    barge_in: AtomicBool,
    /// `config.stt.barge_in_factor`, clamped to at least 1.0 — a factor below
    /// the plain VAD threshold would make the agent's own voice, which the
    /// mic hears at roughly speech level, interrupt every reply it starts.
    barge_in_factor_bits: AtomicU32,
}

impl VadParams {
    pub fn new(stt: &npc_core::config::SttConfig) -> Arc<Self> {
        let params = Arc::new(Self {
            threshold_bits: AtomicU32::new(0f32.to_bits()),
            silence_ms: AtomicU64::new(0),
            barge_in: AtomicBool::new(false),
            barge_in_factor_bits: AtomicU32::new(1f32.to_bits()),
        });
        params.set(stt);
        params
    }

    /// Overwrite every knob from a (possibly hand-edited) config. Negative
    /// and non-finite values are clamped here rather than at the read site,
    /// since a NaN threshold would make every comparison false and silently
    /// deafen the VAD.
    pub fn set(&self, stt: &npc_core::config::SttConfig) {
        let threshold = if stt.input_threshold.is_finite() {
            stt.input_threshold.max(0.0)
        } else {
            0.0
        };
        self.threshold_bits.store(threshold.to_bits(), Ordering::Relaxed);

        let secs = if stt.silence_duration.is_finite() && stt.silence_duration > 0.0 {
            stt.silence_duration
        } else {
            DEFAULT_SILENCE_SECS
        };
        self.silence_ms
            .store((secs * 1000.0).round() as u64, Ordering::Relaxed);

        self.barge_in.store(stt.barge_in, Ordering::Relaxed);
        let factor = if stt.barge_in_factor.is_finite() {
            stt.barge_in_factor.max(1.0)
        } else {
            1.0
        };
        self.barge_in_factor_bits.store(factor.to_bits(), Ordering::Relaxed);
    }

    fn threshold(&self) -> f32 {
        f32::from_bits(self.threshold_bits.load(Ordering::Relaxed))
    }

    fn silence_delay(&self) -> Duration {
        Duration::from_millis(self.silence_ms.load(Ordering::Relaxed))
    }

    pub fn barge_in_enabled(&self) -> bool {
        self.barge_in.load(Ordering::Relaxed)
    }

    /// The RMS a chunk must beat to count as the user talking over a playing
    /// TTS clip. Zero when the plain threshold is zero (threshold disabled),
    /// which the caller reads as "any audio counts".
    pub fn barge_in_threshold(&self) -> f32 {
        self.threshold() * f32::from_bits(self.barge_in_factor_bits.load(Ordering::Relaxed))
    }
}

pub struct VadState {
    sample_rate: u32,
    params: Arc<VadParams>,
    pre_roll_limit: usize,
    start_voice_limit: usize,

    pre_roll: Vec<i16>,
    start_voice: Vec<i16>,
    pcm: Vec<i16>,
    speech_started: bool,
    voiced_samples: usize,
    last_voice_at: Option<Instant>,
    segment_started_at: Option<Instant>,
}

impl VadState {
    pub fn new(sample_rate: u32, params: Arc<VadParams>) -> Self {
        let pre_roll_limit = ms_to_samples(sample_rate, PRE_ROLL_MS);
        let start_voice_limit = ms_to_samples(sample_rate, START_VOICE_MS);

        Self {
            sample_rate,
            params,
            pre_roll_limit,
            start_voice_limit,
            pre_roll: Vec::with_capacity(pre_roll_limit),
            start_voice: Vec::with_capacity(start_voice_limit),
            pcm: Vec::new(),
            speech_started: false,
            voiced_samples: 0,
            last_voice_at: None,
            segment_started_at: None,
        }
    }

    /// Feed one chunk of mono 16-bit PCM samples. Returns `Some(segment)`
    /// when silence (or the max-length cap) just closed out a segment worth
    /// transcribing.
    pub fn process(&mut self, chunk: &[i16]) -> Option<Vec<i16>> {
        // Re-read per chunk rather than caching: this is what makes the web
        // UI's threshold slider audible while you drag it.
        let threshold = self.params.threshold();
        let speaking = threshold <= 0.0 || crate::resample::rms_i16(chunk) > threshold;
        let now = Instant::now();

        if !self.speech_started {
            if speaking {
                self.start_voice.extend_from_slice(chunk);
                if self.start_voice.len() >= self.start_voice_limit {
                    self.pcm.clear();
                    self.pcm.extend_from_slice(&self.pre_roll);
                    self.pre_roll.clear();
                    self.pcm.extend_from_slice(&self.start_voice);
                    self.voiced_samples = self.start_voice.len();
                    self.start_voice.clear();
                    self.speech_started = true;
                    self.last_voice_at = Some(now);
                    self.segment_started_at = Some(now);
                }
            } else {
                self.start_voice.clear();
                append_with_limit(&mut self.pre_roll, chunk, self.pre_roll_limit);
            }
            return None;
        }

        if speaking {
            self.last_voice_at = Some(now);
            self.voiced_samples += chunk.len();
        }
        self.pcm.extend_from_slice(chunk);

        let silence_elapsed = self
            .last_voice_at
            .map(|t| now.duration_since(t))
            .unwrap_or_default();
        let segment_elapsed = self
            .segment_started_at
            .map(|t| now.duration_since(t))
            .unwrap_or_default();

        if silence_elapsed >= self.params.silence_delay() || segment_elapsed.as_secs_f32() >= MAX_SEGMENT_SECS {
            return self.finish_segment();
        }
        None
    }

    /// Called when the capture stream is shutting down, to flush any
    /// in-progress segment (mirrors the Go original's post-loop flush).
    pub fn flush(&mut self) -> Option<Vec<i16>> {
        if self.speech_started && !self.pcm.is_empty() {
            self.finish_segment()
        } else {
            None
        }
    }

    /// While TTS is playing we don't want stray echo turning into a
    /// half-open segment once playback stops; drop any in-progress state.
    pub fn reset(&mut self) {
        self.pre_roll.clear();
        self.start_voice.clear();
        self.pcm.clear();
        self.speech_started = false;
        self.voiced_samples = 0;
        self.last_voice_at = None;
        self.segment_started_at = None;
    }

    fn finish_segment(&mut self) -> Option<Vec<i16>> {
        let voiced_ms = (self.voiced_samples as u64 * 1000) / self.sample_rate.max(1) as u64;
        let pcm = std::mem::take(&mut self.pcm);

        self.pre_roll.clear();
        self.speech_started = false;
        self.voiced_samples = 0;
        self.last_voice_at = None;
        self.segment_started_at = None;

        if pcm.is_empty() || voiced_ms < MIN_VOICED_MS {
            return None;
        }
        let threshold = self.params.threshold();
        if threshold > 0.0 && crate::resample::rms_i16(&pcm) < threshold * MIN_RMS_FACTOR {
            return None;
        }
        Some(pcm)
    }
}

pub(crate) fn ms_to_samples(sample_rate: u32, ms: u64) -> usize {
    ((sample_rate as u64 * ms) / 1000) as usize
}

/// Sliding-window append: keep at most the last `limit` samples of `data`
/// appended to `dst` (used for the pre-roll ring buffer).
fn append_with_limit(dst: &mut Vec<i16>, data: &[i16], limit: usize) {
    if limit == 0 {
        dst.clear();
        return;
    }
    if data.len() >= limit {
        dst.clear();
        dst.extend_from_slice(&data[data.len() - limit..]);
        return;
    }
    dst.extend_from_slice(data);
    if dst.len() > limit {
        let excess = dst.len() - limit;
        dst.drain(0..excess);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud_chunk(n: usize) -> Vec<i16> {
        vec![20000; n]
    }
    fn quiet_chunk(n: usize) -> Vec<i16> {
        vec![0; n]
    }

    /// The two knobs these tests care about, over otherwise-default stt config.
    fn params(threshold: f32, silence: f32) -> Arc<VadParams> {
        VadParams::new(&npc_core::config::SttConfig {
            input_threshold: threshold,
            silence_duration: silence,
            ..Default::default()
        })
    }

    #[test]
    fn short_blip_never_starts_a_segment() {
        let mut vad = VadState::new(16000, params(0.1, 1.5));
        // Well under the 120ms start-voice threshold at 16kHz (1920 samples).
        assert_eq!(vad.process(&loud_chunk(100)), None);
        assert_eq!(vad.process(&quiet_chunk(100)), None);
    }

    #[test]
    fn sustained_speech_then_silence_yields_a_segment() {
        // 50ms silence delay for a fast test.
        let mut vad = VadState::new(16000, params(0.1, 0.05));
        // > 120ms of loud audio to start a segment, > 250ms voiced overall.
        for _ in 0..10 {
            assert_eq!(vad.process(&loud_chunk(1600)), None); // 100ms chunks
        }
        // Now silence long enough to close the segment.
        std::thread::sleep(Duration::from_millis(60));
        let seg = vad.process(&quiet_chunk(100));
        assert!(seg.is_some());
        assert!(!seg.unwrap().is_empty());
    }

    /// The point of holding the knobs in a shared `VadParams`: raising the
    /// threshold from the web UI must silence an already-running VAD without
    /// rebuilding it. `loud_chunk` sits at RMS ~0.61 of full scale, so 0.1
    /// counts as speech and 0.9 does not.
    #[test]
    fn raising_the_threshold_mid_stream_stops_detecting_speech() {
        let knobs = params(0.1, 1.5);
        let mut vad = VadState::new(16000, knobs.clone());

        // Enough loud audio to cross the 120ms start-voice limit.
        for _ in 0..3 {
            vad.process(&loud_chunk(1600));
        }
        assert!(vad.speech_started);

        vad.reset();
        knobs.set(&npc_core::config::SttConfig {
            input_threshold: 0.9,
            silence_duration: 1.5,
            ..Default::default()
        });
        for _ in 0..3 {
            vad.process(&loud_chunk(1600));
        }
        assert!(!vad.speech_started, "same audio should now be below threshold");
    }

    /// A non-positive or non-finite silence_duration (hand-edited config,
    /// or a cleared number field) must fall back to the default rather than
    /// closing every segment instantly.
    #[test]
    fn non_positive_silence_duration_falls_back_to_the_default() {
        let default = Duration::from_secs_f32(DEFAULT_SILENCE_SECS);
        for bad in [0.0, -1.0, f32::NAN] {
            assert_eq!(params(0.1, bad).silence_delay(), default);
        }
        assert_eq!(params(0.1, 0.4).silence_delay(), Duration::from_millis(400));
    }

    /// Barge-in has to be *harder* to trigger than plain speech detection,
    /// otherwise the agent's own voice coming back through the mic ends every
    /// reply it starts. A hand-edited factor below 1.0 is clamped, not obeyed.
    #[test]
    fn barge_in_threshold_never_drops_below_the_vad_threshold() {
        let knobs = params(0.02, 1.0);
        assert!((knobs.barge_in_threshold() - 0.02 * 2.5).abs() < 1e-6);

        knobs.set(&npc_core::config::SttConfig {
            input_threshold: 0.02,
            barge_in_factor: 0.1,
            ..Default::default()
        });
        assert!((knobs.barge_in_threshold() - 0.02).abs() < 1e-6);
    }
}
