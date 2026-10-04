//! WAV encode/decode helpers built on `hound`. Capture encodes 16-bit PCM
//! mono segments to send to the STT endpoint; playback decodes whatever WAV
//! the TTS endpoint returns.
//!
//! Everything tc-npc *produces* is WAV, so [`decode_wav`] stays the path for
//! it. Files the operator supplies are the exception — a chime or BGM track
//! out of the sound library is as likely to be an mp3 as a wav — which is
//! what [`decode_audio`] is for.

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

/// Decode an operator-supplied audio file — WAV or mp3 — to the same
/// `(interleaved f32, sample rate, channels)` shape [`decode_wav`] returns.
///
/// Dispatch is on the container's own magic rather than the file's
/// extension: the extension is the operator's to get wrong (a `.wav` that is
/// really an mp3 is a common export accident), while `RIFF` at byte 0 is not.
/// Anything that isn't RIFF goes to symphonia, whose probe reports its own
/// error if the bytes are neither.
pub fn decode_audio(bytes: &[u8]) -> anyhow::Result<(Vec<f32>, u32, u16)> {
    if bytes.starts_with(b"RIFF") {
        return decode_wav(bytes);
    }
    decode_with_symphonia(bytes)
}

/// The non-WAV half of [`decode_audio`].
///
/// Decodes the file's *default* track only. Multi-track audio is not a case
/// a chime or a BGM clip presents, and picking one track is still better than
/// refusing the file.
fn decode_with_symphonia(bytes: &[u8]) -> anyhow::Result<(Vec<f32>, u32, u16)> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::errors::Error as SymphoniaError;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    // The stream wants an owned, seekable source; the caller's slice is
    // borrowed, so this copy is the price of not reading the file twice.
    let source = Cursor::new(bytes.to_vec());
    let stream = MediaSourceStream::new(Box::new(source), Default::default());
    let probed = symphonia::default::get_probe()
        .format(&Hint::new(), stream, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|err| anyhow::anyhow!("unsupported audio format: {err}"))?;
    let mut format = probed.format;

    let track = format
        .default_track()
        .ok_or_else(|| anyhow::anyhow!("audio file has no decodable track"))?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|err| anyhow::anyhow!("unsupported audio codec: {err}"))?;

    let mut samples: Vec<f32> = Vec::new();
    let mut sample_rate = 0u32;
    let mut channels = 0u16;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            // How symphonia reports a clean end of stream: a plain EOF from
            // the reader, not a distinct variant.
            Err(SymphoniaError::IoError(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymphoniaError::ResetRequired) => {
                // A mid-stream track change. Nothing sane to splice onto what
                // was decoded so far, so stop with what we have.
                break;
            }
            Err(err) => return Err(anyhow::anyhow!("failed to read audio packet: {err}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buffer) => {
                let spec = *buffer.spec();
                sample_rate = spec.rate;
                channels = spec.channels.count() as u16;
                let mut interleaved = SampleBuffer::<f32>::new(buffer.capacity() as u64, spec);
                interleaved.copy_interleaved_ref(buffer);
                samples.extend_from_slice(interleaved.samples());
            }
            // A corrupt frame in an otherwise fine file: skip it rather than
            // abandon the whole clip, which is what symphonia documents these
            // two errors as meaning.
            Err(SymphoniaError::DecodeError(err)) => {
                tracing::debug!(error = %err, "npc-speech: skipping undecodable audio frame");
            }
            Err(SymphoniaError::IoError(err)) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(anyhow::anyhow!("failed to decode audio: {err}")),
        }
    }

    if samples.is_empty() || sample_rate == 0 || channels == 0 {
        anyhow::bail!("audio file decoded to no samples");
    }
    Ok((samples, sample_rate, channels))
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

    /// RIFF magic takes the `hound` path, so a WAV decodes identically
    /// whether it arrives through the operator-file entry point or not.
    #[test]
    fn decode_audio_routes_a_wav_to_the_wav_decoder() {
        let wav = encode_wav_i16(&[0, 1000, -1000], 22050).unwrap();
        let (via_audio, rate, channels) = decode_audio(&wav).unwrap();
        let (via_wav, _, _) = decode_wav(&wav).unwrap();
        assert_eq!(rate, 22050);
        assert_eq!(channels, 1);
        assert_eq!(via_audio, via_wav);
    }

    /// Neither RIFF nor anything symphonia recognizes: an error the caller
    /// can log and skip on, not a panic. (mp3 decoding itself has no unit
    /// test here — there is no encoder in the dependency graph to generate a
    /// fixture with, and checking in a binary one buys little over the
    /// end-to-end path the 予定 tab's test button already exercises.)
    #[test]
    fn decode_audio_errors_on_bytes_that_are_no_known_format() {
        assert!(decode_audio(b"not audio at all, just some bytes").is_err());
        assert!(decode_audio(&[]).is_err());
    }
}
