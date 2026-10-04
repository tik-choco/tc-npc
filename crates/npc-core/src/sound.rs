//! Sound-effect library: the chime/BGM audio files sitting in
//! `{data_dir}/sound/`.
//!
//! The third folder-is-the-library store, after [`crate::vrm`] and
//! [`crate::sprite`], and deliberately the same shape: a plain local folder,
//! entries identified by **file name**, anything dropped in by hand picked up
//! on the next listing, and every name that arrives from an HTTP path
//! validated before it reaches the filesystem.
//!
//! What's different here is that a sound reference is *not* only a library
//! file name. `chime_file`/`bgm_file` in `config.scheduler` predate this
//! folder and have always been read as a plain path — so
//! [`resolve_sound_path`] accepts both: a bare `chime.wav` means the copy in
//! the library, while anything that already worked (an absolute path, or a
//! `sound/chime.wav` written relative to the data dir) keeps working. That
//! is also why resolution deliberately does *not* run
//! [`validate_sound_file_name`]: a config file is written by the operator on
//! their own machine and may legitimately point anywhere, whereas the names
//! the listing endpoints take come from URLs and may not.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Extensions (lowercased) a file must have to be listed as a sound.
///
/// Exactly the formats the playback path can decode — see `npc-speech`'s
/// `wavio::decode_audio`. Listing anything else would put entries in the UI's
/// picker that fail at play time with nothing but a log line to explain it.
const SOUND_EXTENSIONS: [&str; 2] = ["wav", "mp3"];

/// One audio file in the library. Bytes are not included — listing must stay
/// cheap; the playback path reads the one file it is about to play.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoundFile {
    /// File name including the extension — the sound's identity, and what
    /// gets stored in `chime_file`/`bgm_file`.
    pub file: String,
    /// File name without the extension, for display.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
}

/// `{data_dir}/sound` — the library folder.
pub fn sound_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sound")
}

/// Reject anything that isn't a plain `*.wav`/`*.mp3` file name. Same rules
/// as [`crate::vrm::validate_vrm_file_name`], for the same reason: both
/// separators (so a name crafted on one OS can't slip through on another), a
/// Windows drive prefix, `..` anywhere rather than only as a whole component,
/// and a leading dot.
pub fn validate_sound_file_name(file: &str) -> anyhow::Result<()> {
    let invalid = file.is_empty()
        || file.contains('/')
        || file.contains('\\')
        || file.contains("..")
        || file.contains(':')
        || file.starts_with('.');
    if invalid {
        anyhow::bail!("invalid sound file name: {file:?}");
    }
    if !has_sound_extension(Path::new(file)) {
        anyhow::bail!("not a sound file: {file:?}");
    }
    Ok(())
}

/// Case-insensitive extension test against [`SOUND_EXTENSIONS`].
fn has_sound_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SOUND_EXTENSIONS.iter().any(|known| e.eq_ignore_ascii_case(known)))
}

/// Every sound in the library folder, sorted by name. A missing folder yields
/// an empty list rather than an error — an install that has never had a sound
/// added is a normal state, not a fault.
pub fn list_sounds(data_dir: &Path) -> anyhow::Result<Vec<SoundFile>> {
    let dir = sound_dir(data_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || !has_sound_extension(&path) {
            continue;
        }
        let Some(file) = path.file_name().and_then(|n| n.to_str()) else {
            // A name that isn't valid UTF-8 can't survive the JSON round trip
            // the web UI identifies sounds by, so it is skipped rather than
            // listed as something that could never be selected.
            tracing::warn!(path = %path.display(), "skipping sound with non-UTF-8 file name");
            continue;
        };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(SoundFile {
            file: file.to_string(),
            name: path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or(file)
                .to_string(),
            size,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Turn a `chime_file`/`bgm_file` reference into the path to actually read.
///
/// Tried in order, first hit wins:
/// 1. An absolute path is taken as-is — it names one specific file and there
///    is nothing to resolve.
/// 2. `{data_dir}/sound/{reference}` — the library. This is what a bare
///    `chime.wav` picked in the UI means.
/// 3. `{data_dir}/{reference}` — so a `"sound/chime.wav"` written into a
///    config before the library existed still finds the same file.
/// 4. The reference unchanged, i.e. relative to the process's working
///    directory. This is the behavior every reference had before this
///    function existed, kept so no working config breaks.
///
/// A reference that matches nothing comes back as (4), so the caller's
/// "couldn't read it" path reports the operator's own string rather than a
/// guess about which candidate was meant.
pub fn resolve_sound_path(data_dir: &Path, reference: &str) -> PathBuf {
    let as_given = PathBuf::from(reference);
    if reference.is_empty() || as_given.is_absolute() {
        return as_given;
    }
    for candidate in [sound_dir(data_dir).join(reference), data_dir.join(reference)] {
        if candidate.is_file() {
            return candidate;
        }
    }
    as_given
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_data_dir() -> PathBuf {
        std::env::temp_dir().join(format!("npc-core-sound-test-{}", uuid::Uuid::new_v4()))
    }

    fn write_sound(data_dir: &Path, name: &str) {
        let dir = sound_dir(data_dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), b"fake-audio").unwrap();
    }

    #[test]
    fn validate_rejects_traversal_and_separators() {
        for bad in ["../evil.wav", "a/b.wav", "a\\b.wav", "C:chime.wav", "", ".hidden.wav"] {
            assert!(validate_sound_file_name(bad).is_err(), "expected {bad:?} to be rejected");
        }
    }

    #[test]
    fn validate_accepts_the_decodable_extensions_case_insensitively() {
        assert!(validate_sound_file_name("chime.wav").is_ok());
        assert!(validate_sound_file_name("Chime.WAV").is_ok());
        assert!(validate_sound_file_name("bgm.mp3").is_ok());
        assert!(validate_sound_file_name("チャイム.wav").is_ok());
        // Playable elsewhere, but not by this build — see SOUND_EXTENSIONS.
        assert!(validate_sound_file_name("bgm.flac").is_err());
        assert!(validate_sound_file_name("chime").is_err());
    }

    #[test]
    fn list_is_empty_when_folder_is_absent() {
        let dir = temp_data_dir();
        assert!(list_sounds(&dir).unwrap().is_empty());
    }

    /// Anything the operator drops into the folder by hand is part of the
    /// library; files sharing it that this build can't decode are not.
    #[test]
    fn list_reports_decodable_files_sorted_and_ignores_the_rest() {
        let dir = temp_data_dir();
        write_sound(&dir, "zzz.wav");
        write_sound(&dir, "aaa.MP3");
        write_sound(&dir, "notes.txt");
        write_sound(&dir, "track.flac");

        let listed = list_sounds(&dir).unwrap();
        let names: Vec<&str> = listed.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["aaa", "zzz"]);
        assert_eq!(listed[0].file, "aaa.MP3");
        assert_eq!(listed[0].size, 10);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_finds_a_bare_name_in_the_library() {
        let dir = temp_data_dir();
        write_sound(&dir, "chime.wav");
        assert_eq!(resolve_sound_path(&dir, "chime.wav"), sound_dir(&dir).join("chime.wav"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The pre-library spelling: a path written relative to the data dir,
    /// which used to only work when the process happened to be started there.
    #[test]
    fn resolve_finds_a_data_dir_relative_path() {
        let dir = temp_data_dir();
        write_sound(&dir, "chime.wav");
        assert_eq!(resolve_sound_path(&dir, "sound/chime.wav"), dir.join("sound/chime.wav"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nothing matched: hand back exactly what was configured, so the
    /// caller's failure log names the operator's own string.
    #[test]
    fn resolve_returns_the_reference_unchanged_when_nothing_matches() {
        let dir = temp_data_dir();
        assert_eq!(resolve_sound_path(&dir, "absent.wav"), PathBuf::from("absent.wav"));
        assert_eq!(resolve_sound_path(&dir, ""), PathBuf::from(""));
    }

    /// An absolute path names one file; the library must not be consulted
    /// even if a same-named entry happens to sit in it.
    #[test]
    fn resolve_passes_an_absolute_path_through() {
        let dir = temp_data_dir();
        write_sound(&dir, "chime.wav");
        let absolute = std::env::temp_dir().join("chime.wav");
        assert_eq!(
            resolve_sound_path(&dir, &absolute.to_string_lossy()),
            absolute
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
