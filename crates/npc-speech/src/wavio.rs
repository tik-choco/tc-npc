//! WAV encode/decode helpers built on `hound`. Capture encodes 16-bit PCM
//! mono segments to send to the STT endpoint; playback decodes whatever WAV
//! the TTS endpoint returns.

use std::io::Cursor;

/// Encode mono 16-bit PCM samples as a WAV byte buffer.
pub fn encode_wav_i16(samples: &[i16], sample_rate: u32) -> anyhow::Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
        for &s in samples {
            writer.write_sample(s)?;
        }
        writer.finalize()?;
    }
    Ok(cursor.into_inner())
}

/// Decode a WAV byte buffer to normalized `f32` samples (interleaved if
/// multi-channel), plus its sample rate and channel count.
pub fn decode_wav(bytes: &[u8]) -> anyhow::Result<(Vec<f32>, u32, u16)> {
    let mut reader = hound::WavReader::new(Cursor::new(bytes))?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => match spec.bits_per_sample {
            8 => reader
                .samples::<i8>()
                .map(|s| s.unwrap_or(0) as f32 / 128.0)
                .collect(),
            16 => reader
                .samples::<i16>()
                .map(|s| s.unwrap_or(0) as f32 / 32768.0)
                .collect(),
            24 | 32 => {
                let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.unwrap_or(0) as f32 / scale)
                    .collect()
            }
            other => anyhow::bail!("unsupported wav bit depth: {other}"),
        },
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap_or(0.0)).collect(),
    };

    Ok((samples, spec.sample_rate, spec.channels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_pcm16() {
        let samples: Vec<i16> = vec![0, 1000, -1000, 32767, -32768];
        let wav = encode_wav_i16(&samples, 16000).unwrap();
        let (decoded, rate, channels) = decode_wav(&wav).unwrap();
        assert_eq!(rate, 16000);
        assert_eq!(channels, 1);
        assert_eq!(decoded.len(), samples.len());
    }
}
