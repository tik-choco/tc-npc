//! Tiny mono-downmix + linear resampling helpers shared by capture (mic ->
//! 16kHz for STT) and playback (TTS wav's native rate -> output device
//! rate). Deliberately simple: linear interpolation, no anti-aliasing
//! filter, and (for the capture path) no state carried across callback
//! buffers. That is enough for VAD/STT purposes and for short TTS clips; see
//! the module report for the tradeoffs.

/// Average all channels of an interleaved `[f32]` buffer down to mono.
/// `channels <= 1` returns the input as-is (already mono, or silence).
pub fn downmix_to_mono(data: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    let ch = channels as usize;
    data.chunks(ch)
        .map(|frame| frame.iter().sum::<f32>() / ch as f32)
        .collect()
}

/// Linearly resample `input` (assumed `src_rate` Hz) to `dst_rate` Hz.
/// A no-op copy when the rates already match.
pub fn resample_linear(input: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if input.is_empty() || src_rate == 0 || dst_rate == 0 || src_rate == dst_rate {
        return input.to_vec();
    }

    let ratio = src_rate as f64 / dst_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor().max(0.0) as usize;
    let mut out = Vec::with_capacity(out_len);
    let last_idx = input.len() - 1;

    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = (src_pos - idx as f64) as f32;
        let s0 = input[idx.min(last_idx)];
        let s1 = input[(idx + 1).min(last_idx)];
        out.push(s0 + (s1 - s0) * frac);
    }

    out
}

/// RMS of a normalized `[-1.0, 1.0]` f32 sample buffer.
pub fn rms_f32(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// RMS of a 16-bit PCM sample buffer, normalized the same way the Go
/// `agent-speech` original did (`val / 32768.0`).
pub fn rms_i16(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples
        .iter()
        .map(|&s| {
            let n = s as f64 / 32768.0;
            n * n
        })
        .sum();
    (sum / samples.len() as f64).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_noop_when_rates_match() {
        let input = vec![0.1, 0.2, 0.3];
        assert_eq!(resample_linear(&input, 16000, 16000), input);
    }

    #[test]
    fn resample_downsamples_roughly_to_expected_length() {
        let input = vec![0.0f32; 4800];
        let out = resample_linear(&input, 48000, 16000);
        assert_eq!(out.len(), 1600);
    }

    #[test]
    fn downmix_stereo_averages_channels() {
        let data = vec![1.0, -1.0, 0.5, 0.5];
        let mono = downmix_to_mono(&data, 2);
        assert_eq!(mono, vec![0.0, 0.5]);
    }
}
