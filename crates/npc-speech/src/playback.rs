//! TTS playback thread: owns the cpal output stream (also `!Send`, also
//! lives on its own dedicated OS thread). Consumes [`PlaybackCmd`]s from a
//! `std::sync::mpsc` channel fed by the async bus-subscriber tasks in
//! `lib.rs`, queueing normal `chat_response` clips and letting
//! `agent:interrupt` "tts" messages jump the queue.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use npc_core::config::SpeechConfig;

use crate::device::select_output_device;
use crate::resample::resample_linear;
use crate::wavio::decode_wav;

/// Extra silence appended after the last sample is handed to the device, so
/// we don't cut the tail off before the device's internal buffer has
/// actually played it.
const TAIL_LATENCY: Duration = Duration::from_millis(120);
const POLL_INTERVAL: Duration = Duration::from_millis(15);

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

/// Entry point run on a dedicated `std::thread`.
pub fn run_playback_thread(speech: SpeechConfig, rx: Receiver<PlaybackCmd>, is_playing: Arc<AtomicBool>) {
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

    let mut queue: VecDeque<Vec<u8>> = VecDeque::new();

    'outer: loop {
        if let Some(wav) = queue.pop_front() {
            match play_wav_blocking(&device, &wav, &rx, &mut queue, &is_playing) {
                PlayOutcome::Shutdown => break 'outer,
                PlayOutcome::Done => {}
            }
            continue;
        }

        match rx.recv() {
            Ok(PlaybackCmd::Enqueue(wav)) => queue.push_back(wav),
            Ok(PlaybackCmd::Priority(items)) => {
                queue.clear();
                queue.extend(items);
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
    queue: &mut VecDeque<Vec<u8>>,
    is_playing: &Arc<AtomicBool>,
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

    is_playing.store(true, Ordering::SeqCst);
    let mut outcome = PlayOutcome::Done;

    loop {
        if finished.load(Ordering::SeqCst) {
            break;
        }
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(PlaybackCmd::Enqueue(w)) => queue.push_back(w),
            Ok(PlaybackCmd::Priority(items)) => {
                queue.clear();
                queue.extend(items);
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
    is_playing.store(false, Ordering::SeqCst);

    // Drain any commands that arrived in the same instant playback ended,
    // without blocking further.
    loop {
        match rx.try_recv() {
            Ok(PlaybackCmd::Enqueue(w)) => queue.push_back(w),
            Ok(PlaybackCmd::Priority(items)) => {
                queue.clear();
                queue.extend(items);
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
