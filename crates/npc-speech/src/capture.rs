//! Mic capture thread: owns the cpal input stream (cpal streams are `!Send`,
//! so this lives on its own dedicated OS thread, never touching tokio),
//! resamples/downmixes to mono `config.speech.input_sample_rate`, runs it
//! through [`crate::vad::VadState`], and reports events back to the async
//! world over an unbounded tokio channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, StreamTrait};
use npc_core::config::{SpeechConfig, SttConfig};
use tokio::sync::mpsc::UnboundedSender;

use crate::device::select_input_device;
use crate::resample::{downmix_to_mono, resample_linear, rms_f32};
use crate::vad::VadState;
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
}

/// Entry point run on a dedicated `std::thread`. Logs and returns on setup
/// failure (e.g. no input device) rather than panicking the thread.
pub fn run_capture_thread(
    speech: SpeechConfig,
    stt: SttConfig,
    events_tx: UnboundedSender<CaptureEvent>,
    stop: Arc<AtomicBool>,
    tts_playing: Arc<AtomicBool>,
) {
    if let Err(err) = run_capture_inner(speech, stt, events_tx, stop, tts_playing) {
        tracing::error!(error = %err, "npc-speech: capture thread failed");
    }
}

fn run_capture_inner(
    speech: SpeechConfig,
    stt: SttConfig,
    events_tx: UnboundedSender<CaptureEvent>,
    stop: Arc<AtomicBool>,
    tts_playing: Arc<AtomicBool>,
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

    let vad = Arc::new(Mutex::new(VadState::new(
        target_rate,
        stt.input_threshold,
        stt.silence_duration,
    )));
    let last_volume_publish = Arc::new(Mutex::new(
        Instant::now()
            .checked_sub(VOLUME_PUBLISH_INTERVAL)
            .unwrap_or_else(Instant::now),
    ));

    let err_fn = |err| tracing::error!(error = %err, "npc-speech: input stream error");

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let vad = vad.clone();
            let events_tx = events_tx.clone();
            let tts_playing = tts_playing.clone();
            let last_volume_publish = last_volume_publish.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mono = downmix_to_mono(data, channels);
                    handle_input_chunk(
                        mono,
                        device_rate,
                        target_rate,
                        &vad,
                        &events_tx,
                        &tts_playing,
                        &last_volume_publish,
                    );
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let vad = vad.clone();
            let events_tx = events_tx.clone();
            let tts_playing = tts_playing.clone();
            let last_volume_publish = last_volume_publish.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    let f32_data: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                    let mono = downmix_to_mono(&f32_data, channels);
                    handle_input_chunk(
                        mono,
                        device_rate,
                        target_rate,
                        &vad,
                        &events_tx,
                        &tts_playing,
                        &last_volume_publish,
                    );
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
    if let Some(pcm) = vad.lock().unwrap().flush() {
        if let Ok(wav) = encode_wav_i16(&pcm, target_rate) {
            let _ = events_tx.send(CaptureEvent::Segment(wav));
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_input_chunk(
    mono_device_rate: Vec<f32>,
    device_rate: u32,
    target_rate: u32,
    vad: &Mutex<VadState>,
    events_tx: &UnboundedSender<CaptureEvent>,
    tts_playing: &AtomicBool,
    last_volume_publish: &Mutex<Instant>,
) {
    let resampled = if device_rate == target_rate {
        mono_device_rate
    } else {
        resample_linear(&mono_device_rate, device_rate, target_rate)
    };
    if resampled.is_empty() {
        return;
    }

    // Mic level, throttled to roughly VOLUME_PUBLISH_INTERVAL.
    {
        let mut last = last_volume_publish.lock().unwrap();
        if last.elapsed() >= VOLUME_PUBLISH_INTERVAL {
            *last = Instant::now();
            let level = ((rms_f32(&resampled) as f64) * VOLUME_UI_GAIN).clamp(0.0, 1.0);
            let _ = events_tx.send(CaptureEvent::Volume(level));
        }
    }

    // Echo prevention: while TTS is actively playing, don't run mic audio
    // through the VAD/STT pipeline at all.
    if tts_playing.load(Ordering::Relaxed) {
        vad.lock().unwrap().reset();
        return;
    }

    let pcm_i16: Vec<i16> = resampled
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect();

    let segment = { vad.lock().unwrap().process(&pcm_i16) };
    if let Some(pcm) = segment {
        match encode_wav_i16(&pcm, target_rate) {
            Ok(wav) => {
                let _ = events_tx.send(CaptureEvent::Segment(wav));
            }
            Err(err) => tracing::error!(error = %err, "npc-speech: failed to encode wav segment"),
        }
    }
}
