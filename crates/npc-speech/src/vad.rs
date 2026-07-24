//! RMS-based voice-activity segmentation, ported from the OpenAI-STT path of
//! Go `agent-speech` (`streamOpenAISTTAudio` in `internal/app/app.go`).
//!
//! Operates on mono 16-bit PCM chunks already resampled to the target rate
//! (see `resample.rs`). Each `process()` call corresponds to one audio
//! capture buffer (analogous to one `<-cap.DataChan()` receive in the Go
//! original).

use std::time::{Duration, Instant};

/// Pre-roll ring buffer kept before speech is detected, so the segment sent
/// to STT includes a little audio from just before the threshold was
/// crossed (Go: `STTPreRollDuration`).
const PRE_ROLL_MS: u64 = 300;
/// Consecutive above-threshold audio required before a segment is
/// considered "started" (Go: `STTStartVoiceDuration`).
const START_VOICE_MS: u64 = 120;
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

pub struct VadState {
    sample_rate: u32,
    threshold: f32,
    silence_delay: Duration,
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
    pub fn new(sample_rate: u32, threshold: f32, silence_duration_secs: f32) -> Self {
        let silence_delay = if silence_duration_secs <= 0.0 {
            Duration::from_secs_f32(DEFAULT_SILENCE_SECS)
        } else {
            Duration::from_secs_f32(silence_duration_secs)
        };
        let pre_roll_limit = ms_to_samples(sample_rate, PRE_ROLL_MS);
        let start_voice_limit = ms_to_samples(sample_rate, START_VOICE_MS);

        Self {
            sample_rate,
            threshold,
            silence_delay,
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
        let speaking = self.threshold <= 0.0 || crate::resample::rms_i16(chunk) > self.threshold;
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

        if silence_elapsed >= self.silence_delay || segment_elapsed.as_secs_f32() >= MAX_SEGMENT_SECS {
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
        if self.threshold > 0.0 && crate::resample::rms_i16(&pcm) < self.threshold * MIN_RMS_FACTOR {
            return None;
        }
        Some(pcm)
    }
}

fn ms_to_samples(sample_rate: u32, ms: u64) -> usize {
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

    #[test]
    fn short_blip_never_starts_a_segment() {
        let mut vad = VadState::new(16000, 0.1, 1.5);
        // Well under the 120ms start-voice threshold at 16kHz (1920 samples).
        assert_eq!(vad.process(&loud_chunk(100)), None);
        assert_eq!(vad.process(&quiet_chunk(100)), None);
    }

    #[test]
    fn sustained_speech_then_silence_yields_a_segment() {
        let mut vad = VadState::new(16000, 0.1, 0.05); // 50ms silence delay for a fast test
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
}
