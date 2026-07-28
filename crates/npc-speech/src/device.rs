//! cpal device selection: case-insensitive substring match against
//! `config.speech.{input,output}_device`, falling back to the host's
//! default device when empty/unmatched.
//!
//! Also enumerates the host's devices for the web UI's 音声 device pickers
//! ([`list_devices`]) so picking a device doesn't mean typing a name blind.

use std::collections::HashSet;

use cpal::traits::{DeviceTrait, HostTrait};
use serde::Serialize;

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

/// What the host can see right now, for the UI's device pickers. The
/// `default_*` names are only there so the picker can label its "OS default"
/// entry with the device that entry will actually resolve to — an empty
/// `config.speech.{input,output}_device` still means "ask the host at
/// startup", not "pin this name".
#[derive(Debug, Default, Clone, Serialize)]
pub struct AudioDevices {
    pub input: Vec<String>,
    pub output: Vec<String>,
    pub default_input: Option<String>,
    pub default_output: Option<String>,
}

/// Enumerate the host's input/output devices. Blocking — cpal talks to the
/// OS audio API (WASAPI on Windows), so callers on the tokio runtime must
/// wrap this in `spawn_blocking`.
///
/// Enumeration failures degrade to an empty list rather than an error: a
/// machine with no mic should still get an output picker, and the UI's
/// fall-back-to-manual path handles the empty case.
pub fn list_devices() -> AudioDevices {
    let host = cpal::default_host();
    AudioDevices {
        input: collect_names(host.input_devices().ok()),
        output: collect_names(host.output_devices().ok()),
        default_input: host.default_input_device().and_then(|d| d.name().ok()),
        default_output: host.default_output_device().and_then(|d| d.name().ok()),
    }
}

/// Named devices in host order, dropping unnamed ones and duplicates. Hosts
/// do report the same name twice (the same endpoint reached through two
/// interfaces), and a duplicate is meaningless in a picker whose value *is*
/// the name.
fn collect_names(devices: Option<impl Iterator<Item = cpal::Device>>) -> Vec<String> {
    let mut seen = HashSet::new();
    devices
        .into_iter()
        .flatten()
        .filter_map(|d| d.name().ok())
        .filter(|name| !name.trim().is_empty())
        .filter(|name| seen.insert(name.clone()))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Enumeration must never panic and must never hand the UI a duplicate
    /// or blank name — the picker's option value *is* the name, so either
    /// would produce an unselectable entry. A machine with no audio hardware
    /// (CI) legitimately yields empty lists, so emptiness isn't asserted.
    #[test]
    fn list_devices_returns_unique_named_devices() {
        let devices = list_devices();
        for names in [&devices.input, &devices.output] {
            let unique: HashSet<&String> = names.iter().collect();
            assert_eq!(unique.len(), names.len(), "duplicate device name in {names:?}");
            assert!(names.iter().all(|n| !n.trim().is_empty()));
        }
    }
}
