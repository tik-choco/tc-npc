//! Offline BGM-under-TTS precompute for `agent:interrupt` "tts" announcements
//! (ports Go `agent-scheduler`'s `announcementMixer`).
//!
//! The Go original mixed live, pulling samples from a stream as playback
//! advanced. tc-npc's playback queue (`playback::PlaybackCmd`) instead plays
//! whole WAV clips back-to-back, and an announcement's TTS length is already
//! known the moment synthesis returns — so [`mix_announcement`] precomputes
//! the entire mixed buffer up front and hands back one finished WAV, and
//! `playback.rs` never has to know BGM mixing happened at all.

use crate::resample::{downmix_to_mono, resample_linear};
use crate::wavio::{decode_wav, encode_wav_i16};

/// Background-music parameters for [`mix_announcement`], already read from
/// disk.
pub struct BgmMix {
    /// Raw wav file bytes.
    pub bytes: Vec<u8>,
    /// Resolved linear gain (already defaulted upstream — no further "0
    /// means use a default" logic applies here).
    pub volume: f32,
    pub play_full: bool,
    /// `""` or `"M:SS"`.
    pub end_time: String,
}

/// BGM gain factor while the TTS is still speaking, relative to
/// `BgmMix::volume` (i.e. the BGM plays at half its target volume while
/// ducked).
const DUCK_TARGET: f32 = 0.5;
/// One-pole IIR smoothing coefficient for the duck transition: applied once
/// per sample as `current += (target - current) * DUCK_SMOOTHING`, so the
/// duck/undock is a smooth ease rather than a hard cut.
const DUCK_SMOOTHING: f32 = 0.05;
/// Length of the linear fade-to-silence applied once the BGM needs to stop
/// (TTS finished and `play_full` is false, or `bgm_end_time` was reached).
const FADE_SECONDS: f32 = 2.0;

/// Decode `tts_wav`, scale it by `volume`, optionally mix `bgm` underneath
/// with ducking + fade-out, and re-encode to WAV bytes ready to hand to the
/// playback queue as one clip.
///
/// Behavior (see the module doc for the full spec this ports):
/// - No BGM: the TTS is simply scaled by `volume`.
/// - With BGM: the BGM plays under the TTS at `bgm.volume`, ducked to half
///   that while the TTS is still speaking (smoothed, not a hard cut), and
///   back to full once the TTS finishes.
///   - `bgm.play_full == false` (the common case): once the TTS finishes,
///     the BGM fades linearly to silence over [`FADE_SECONDS`], then stops.
///   - `bgm.play_full == true`: the BGM keeps playing at full volume after
///     the TTS finishes until the BGM clip itself runs out (no looping).
///   - Independently, if `bgm.end_time` ("M:SS") is set, once total elapsed
///     playback reaches that point the [`FADE_SECONDS`] fade-out is forced
///     regardless of `play_full` or whether the TTS is still speaking.
/// - The TTS itself is always played at full presence, scaled only by
///   `volume` — it is never ducked or faded.
pub fn mix_announcement(tts_wav: &[u8], volume: f32, bgm: Option<BgmMix>) -> anyhow::Result<Vec<u8>> {
    let (tts_raw, tts_rate, tts_channels) = decode_wav(tts_wav)?;
    let tts_mono = if tts_channels > 1 { downmix_to_mono(&tts_raw, tts_channels) } else { tts_raw };
    let tts_len = tts_mono.len();

    let output: Vec<f32> = match bgm {
        None => tts_mono.iter().map(|s| (s * volume).clamp(-1.0, 1.0)).collect(),
        Some(bgm) => {
            let (bgm_raw, bgm_rate, bgm_channels) = decode_wav(&bgm.bytes)?;
            let bgm_mono_src = if bgm_channels > 1 { downmix_to_mono(&bgm_raw, bgm_channels) } else { bgm_raw };
            // Resampled onto the TTS's sample clock so both buffers can be
            // indexed by the same `i` for the rest of this function.
            let bgm_mono = resample_linear(&bgm_mono_src, bgm_rate, tts_rate);
            let bgm_len = bgm_mono.len();

            let max_duration_samples = parse_mmss(&bgm.end_time).map(|secs| (secs * tts_rate as f32) as usize);
            // Guarded against a (never-expected-in-practice) zero sample
            // rate: a fade window of zero samples would divide by zero below.
            let fade_samples = ((FADE_SECONDS * tts_rate as f32) as usize).max(1);

            let mut out = Vec::new();
            let mut current_bgm_factor = 1.0f32;
            let mut fade_start: Option<usize> = None;

            let mut i = 0usize;
            loop {
                let tts_done = i >= tts_len;
                let bgm_done = i >= bgm_len;

                let forced_cutoff = max_duration_samples.is_some_and(|m| i >= m);
                // Both sources exhausted and nothing is forcing an early
                // stop: `play_full` says to let the BGM run to its own end,
                // which it has now done, so there is nothing left to emit.
                if tts_done && bgm_done && bgm.play_full && !forced_cutoff {
                    break;
                }

                let should_fade = (!bgm.play_full && tts_done) || forced_cutoff;
                if should_fade && fade_start.is_none() {
                    fade_start = Some(i);
                }

                // Ducking is driven purely by whether the TTS is still
                // speaking, independent of any fade — this is what makes the
                // BGM ease back toward full volume the instant the TTS ends
                // even while a forced (`bgm_end_time`) fade is already
                // running.
                let target_factor = if tts_done { 1.0 } else { DUCK_TARGET };
                current_bgm_factor += (target_factor - current_bgm_factor) * DUCK_SMOOTHING;

                // The fade envelope multiplies on top of the duck factor,
                // rather than replacing it, so a forced cutoff mid-speech
                // fades out from the ducked level instead of jumping back to
                // full volume first.
                let mut sample_factor = current_bgm_factor;
                if let Some(start) = fade_start {
                    let fade_ratio = 1.0 - (i - start) as f32 / fade_samples as f32;
                    if fade_ratio <= 0.0 {
                        break; // Fully faded — stop here, nothing more to emit.
                    }
                    sample_factor *= fade_ratio;
                }

                let tts_s = if i < tts_len { tts_mono[i] * volume } else { 0.0 };
                let bgm_s = if i < bgm_len { bgm_mono[i] * bgm.volume * sample_factor } else { 0.0 };
                out.push((tts_s + bgm_s).clamp(-1.0, 1.0));

                i += 1;
                // Safety valve: never produce more than ~10 minutes of audio
                // even if inputs are pathological (e.g. a huge BGM file with
                // `play_full` and no `end_time`).
                if i > tts_rate as usize * 600 {
                    break;
                }
            }
            out
        }
    };

    let output_i16: Vec<i16> = output.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    encode_wav_i16(&output_i16, tts_rate)
}

/// Parses `"M:SS"` (minutes:seconds) into total seconds. Returns `None` for
/// `""` or anything that isn't exactly two colon-separated integers (fails
/// closed rather than silently producing `0`, unlike the Go original's
/// lenient `Sscanf`).
fn parse_mmss(s: &str) -> Option<f32> {
    let parts: Vec<&str> = s.split(':').collect();
    match parts.as_slice() {
        [m, s] => {
            let minutes: f32 = m.parse().ok()?;
            let seconds: f32 = s.parse().ok()?;
            Some(minutes * 60.0 + seconds)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------
    // mix_announcement
    // -------------------------------------------------------------

    #[test]
    fn no_bgm_just_scales_tts_by_volume() {
        let rate = 8000;
        let samples: Vec<i16> = vec![20000, -20000, 15000, -15000, 0];
        let tts_wav = encode_wav_i16(&samples, rate).unwrap();
        let (orig, _, _) = decode_wav(&tts_wav).unwrap();

        let mixed = mix_announcement(&tts_wav, 0.5, None).unwrap();
        let (out, out_rate, out_channels) = decode_wav(&mixed).unwrap();

        assert_eq!(out_rate, rate);
        assert_eq!(out_channels, 1);
        assert_eq!(out.len(), orig.len());
        for (o, s) in out.iter().zip(orig.iter()) {
            assert!((o - s * 0.5).abs() < 0.001, "o={o} expected={}", s * 0.5);
        }
    }

    #[test]
    fn short_bgm_produces_silent_tail_without_panicking() {
        let rate = 100u32;
        let tts_samples = vec![10000i16; 50];
        let bgm_samples = vec![10000i16; 10]; // much shorter than tts + fade window
        let tts_wav = encode_wav_i16(&tts_samples, rate).unwrap();
        let bgm_wav = encode_wav_i16(&bgm_samples, rate).unwrap();

        let bgm = BgmMix {
            bytes: bgm_wav,
            volume: 0.8,
            play_full: false,
            end_time: String::new(),
        };
        let mixed = mix_announcement(&tts_wav, 1.0, Some(bgm)).unwrap();
        let (out, out_rate, _) = decode_wav(&mixed).unwrap();

        let fade_samples = (FADE_SECONDS * rate as f32) as usize;
        assert_eq!(out_rate, rate);
        // tts plays in full, then fades out for FADE_SECONDS with the (long
        // since exhausted) bgm contributing silence for the tail.
        assert_eq!(out.len(), tts_samples.len() + fade_samples);
        for &s in &out[bgm_samples.len().max(tts_samples.len())..] {
            // Both tts and bgm are fully exhausted by this point, so this
            // must be exact silence, not just "quiet".
            assert_eq!(s, 0.0, "unexpected non-silent tail sample: {s}");
        }
    }

    #[test]
    fn play_full_with_longer_bgm_extends_past_tts_len() {
        let rate = 100u32;
        let tts_samples = vec![5000i16; 20];
        let bgm_samples = vec![8000i16; 100]; // longer than tts
        let tts_wav = encode_wav_i16(&tts_samples, rate).unwrap();
        let bgm_wav = encode_wav_i16(&bgm_samples, rate).unwrap();

        let bgm = BgmMix {
            bytes: bgm_wav,
            volume: 1.0,
            play_full: true,
            end_time: String::new(),
        };
        let mixed = mix_announcement(&tts_wav, 1.0, Some(bgm)).unwrap();
        let (out, _, _) = decode_wav(&mixed).unwrap();

        // No end_time and play_full=true: runs until the bgm itself is
        // exhausted, well past the (shorter) tts.
        assert_eq!(out.len(), bgm_samples.len());
        assert!(out.len() > tts_samples.len());
    }

    #[test]
    fn play_full_false_fades_rather_than_cutting_abruptly() {
        let rate = 100u32;
        let tts_samples = vec![5000i16; 30];
        let bgm_samples = vec![10000i16; 300]; // plenty left to fade
        let tts_wav = encode_wav_i16(&tts_samples, rate).unwrap();
        let bgm_wav = encode_wav_i16(&bgm_samples, rate).unwrap();

        let bgm = BgmMix {
            bytes: bgm_wav,
            volume: 1.0,
            play_full: false,
            end_time: String::new(),
        };
        let mixed = mix_announcement(&tts_wav, 1.0, Some(bgm)).unwrap();
        let (out, _, _) = decode_wav(&mixed).unwrap();

        let fade_samples = (FADE_SECONDS * rate as f32) as usize;
        // Not an abrupt cut at tts_len: playback continues through the fade
        // window before stopping.
        assert_eq!(out.len(), tts_samples.len() + fade_samples);

        let early = out[tts_samples.len() + 5].abs();
        let late = out[out.len() - 2].abs();
        assert!(early > 0.05, "expected bgm still audible just after tts ends, got {early}");
        assert!(late < 0.02, "expected near-silence at the end of the fade, got {late}");
        assert!(early > late * 3.0, "expected a gradual taper: early={early} late={late}");
    }

    #[test]
    fn output_is_always_clamped_to_valid_range() {
        // Loud tts + loud bgm at full (undocked, post-tts) volume must not
        // wrap/overflow when summed.
        let rate = 100u32;
        let tts_samples = vec![32000i16; 10];
        let bgm_samples = vec![32000i16; 10];
        let tts_wav = encode_wav_i16(&tts_samples, rate).unwrap();
        let bgm_wav = encode_wav_i16(&bgm_samples, rate).unwrap();

        let bgm = BgmMix {
            bytes: bgm_wav,
            volume: 1.0,
            play_full: true,
            end_time: String::new(),
        };
        let mixed = mix_announcement(&tts_wav, 1.0, Some(bgm)).unwrap();
        let (out, _, _) = decode_wav(&mixed).unwrap();
        for &s in &out {
            assert!((-1.0..=1.0).contains(&s), "sample out of range: {s}");
        }
    }

    // -------------------------------------------------------------
    // parse_mmss
    // -------------------------------------------------------------

    #[test]
    fn parse_mmss_parses_minutes_and_seconds() {
        assert_eq!(parse_mmss("1:30"), Some(90.0));
        assert_eq!(parse_mmss("0:05"), Some(5.0));
        assert_eq!(parse_mmss("2:00"), Some(120.0));
    }

    #[test]
    fn parse_mmss_rejects_empty_and_garbage() {
        assert_eq!(parse_mmss(""), None);
        assert_eq!(parse_mmss("garbage"), None);
        assert_eq!(parse_mmss("1:2:3"), None);
        assert_eq!(parse_mmss("1:xx"), None);
        assert_eq!(parse_mmss(":"), None);
    }
}
