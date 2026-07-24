//! cpal device selection: case-insensitive substring match against
//! `config.speech.{input,output}_device`, falling back to the host's
//! default device when empty/unmatched.

use cpal::traits::{DeviceTrait, HostTrait};

pub fn select_input_device(host: &cpal::Host, name_substr: &str) -> anyhow::Result<cpal::Device> {
    if let Ok(devices) = host.input_devices() {
        if let Some(d) = find_by_substr(devices, name_substr) {
            return Ok(d);
        }
    }
    host.default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no input audio device available"))
}

pub fn select_output_device(host: &cpal::Host, name_substr: &str) -> anyhow::Result<cpal::Device> {
    if let Ok(devices) = host.output_devices() {
        if let Some(d) = find_by_substr(devices, name_substr) {
            return Ok(d);
        }
    }
    host.default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no output audio device available"))
}

fn find_by_substr(devices: impl Iterator<Item = cpal::Device>, name_substr: &str) -> Option<cpal::Device> {
    let needle = name_substr.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    devices
        .filter_map(|d| d.name().ok().map(|n| (n, d)))
        .find(|(n, _)| n.to_lowercase().contains(&needle))
        .map(|(_, d)| d)
}
