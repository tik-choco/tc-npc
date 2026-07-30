//! VRM avatar model library: the `.vrm` files sitting in `{data_dir}/vrm/`.
//!
//! tc-npc runs on the same machine as its operator, so the model library is
//! simply a local folder rather than a database: anything dropped into
//! `{data_dir}/vrm/` by hand shows up in the キャラ tab's model list on the
//! next load, and the web UI's "add" button is nothing more than a way to
//! copy a file into that folder without leaving the browser. (tc-town, which
//! is a pure browser app with no server to write files with, has to keep the
//! same models in IndexedDB instead — the two libraries are deliberately not
//! shared, since they don't even live on the same origin.)
//!
//! A model is identified by its **file name**, which is also what a
//! `Character`'s `avatar` field stores (see [`crate::character::Avatar`]).
//! Every entry point therefore runs the name through
//! [`validate_vrm_file_name`] first: these names arrive from HTTP paths, and
//! a `..` or a separator in one would otherwise read or delete a file
//! anywhere on disk.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The extension (lowercased) a file must have to be treated as a model.
const VRM_EXTENSION: &str = "vrm";

/// One `.vrm` in the library. The bytes are deliberately not included —
/// listing the folder must stay cheap no matter how large the models are;
/// [`read_vrm`] fetches those for the one model actually being displayed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VrmModel {
    /// File name including the `.vrm` extension — the model's identity.
    pub file: String,
    /// File name without the extension, for display.
    pub name: String,
    /// Size in bytes, so the UI can warn about a model too heavy to render.
    pub size: u64,
}

/// `{data_dir}/vrm` — the library folder.
pub fn vrm_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("vrm")
}

/// Reject anything that isn't a plain `*.vrm` file name.
///
/// Both separators are checked (not just the platform's own) so a name
/// crafted on one OS can't slip through on another, and a Windows drive
/// prefix (`C:`) is rejected along with them. `..` is refused outright
/// rather than only as a whole component — no legitimate model name needs
/// it, so the blunt check has no false positives to pay for.
pub fn validate_vrm_file_name(file: &str) -> anyhow::Result<()> {
    let invalid = file.is_empty()
        || file.contains('/')
        || file.contains('\\')
        || file.contains("..")
        || file.contains(':')
        || file.starts_with('.');
    if invalid {
        anyhow::bail!("invalid vrm file name: {file:?}");
    }
    if !has_vrm_extension(Path::new(file)) {
        anyhow::bail!("not a .vrm file: {file:?}");
    }
    Ok(())
}

/// Case-insensitive `.vrm` extension test — models are routinely named
/// `Model.VRM` by the tools that export them.
fn has_vrm_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(VRM_EXTENSION))
}

fn vrm_path(data_dir: &Path, file: &str) -> PathBuf {
    vrm_dir(data_dir).join(file)
}

/// Every `.vrm` in the library folder, sorted by name. A missing folder
/// yields an empty list rather than an error — an install that has never had
/// a model added is a normal state, not a fault.
pub fn list_vrm_models(data_dir: &Path) -> anyhow::Result<Vec<VrmModel>> {
    let dir = vrm_dir(data_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || !has_vrm_extension(&path) {
            continue;
        }
        let Some(file) = path.file_name().and_then(|n| n.to_str()) else {
            // A name that isn't valid UTF-8 can't survive the JSON round trip
            // the web UI identifies models by, so it's skipped rather than
            // listed as something that could never be loaded.
            tracing::warn!(path = %path.display(), "skipping vrm with non-UTF-8 file name");
            continue;
        };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(VrmModel {
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

/// Whether a model of this name is in the library. `Ok(false)` covers "not
/// there"; an invalid name is still an `Err`, since that's a caller mistake
/// rather than an answer about the folder.
///
/// Separate from [`read_vrm`] because callers that only need to know a
/// reference resolves (validating an avatar assignment, deciding what to put
/// in a `hello` frame) would otherwise read tens of megabytes to find out.
pub fn vrm_exists(data_dir: &Path, file: &str) -> anyhow::Result<bool> {
    validate_vrm_file_name(file)?;
    Ok(vrm_path(data_dir, file).is_file())
}

/// Cache-validator inputs for one model's bytes: size and last-modified
/// time. A conditional `GET` (see `npc-server`'s `api_get_vrm_file`) needs
/// exactly these two numbers to build an `ETag` and decide whether a
/// previously-fetched copy is still good — reading the 10-50MB file just to
/// answer that would defeat the entire point of a validator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VrmFileStat {
    /// File size in bytes.
    pub size: u64,
    /// Last-modified time straight from the filesystem. A same-size
    /// overwrite (re-uploading a model under its existing file name, which
    /// is exactly what the キャラ tab's "replace" does) still advances this,
    /// which is why `size` alone isn't a sufficient validator.
    pub modified: std::time::SystemTime,
}

/// Size and modified time for one model in the library. `Ok(None)` covers
/// "not there" (the same non-error treatment [`vrm_exists`] gives its
/// `Ok(false)` and [`read_vrm`] gives its `Ok(None)`); an invalid name is
/// still `Err`, since that's a caller mistake rather than an answer about
/// the folder.
pub fn vrm_file_stat(data_dir: &Path, file: &str) -> anyhow::Result<Option<VrmFileStat>> {
    validate_vrm_file_name(file)?;
    let path = vrm_path(data_dir, file);
    let meta = match std::fs::metadata(&path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(anyhow::anyhow!("failed to stat vrm {file}: {err}")),
    };
    if !meta.is_file() {
        return Ok(None);
    }
    let modified = meta
        .modified()
        .map_err(|err| anyhow::anyhow!("failed to read modified time for vrm {file}: {err}"))?;
    Ok(Some(VrmFileStat { size: meta.len(), modified }))
}

/// Raw `.vrm` bytes for one model, or `Ok(None)` if no such file exists.
pub fn read_vrm(data_dir: &Path, file: &str) -> anyhow::Result<Option<Vec<u8>>> {
    validate_vrm_file_name(file)?;
    let path = vrm_path(data_dir, file);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(|e| anyhow::anyhow!("failed to read vrm {file}: {e}"))?;
    Ok(Some(bytes))
}

/// Write `bytes` into the library as `file`, creating the folder if needed
/// and overwriting any model already under that name. Returns the stored
/// entry.
pub fn save_vrm(data_dir: &Path, file: &str, bytes: &[u8]) -> anyhow::Result<VrmModel> {
    validate_vrm_file_name(file)?;
    let dir = vrm_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let path = vrm_path(data_dir, file);
    std::fs::write(&path, bytes).map_err(|e| anyhow::anyhow!("failed to write vrm {file}: {e}"))?;
    Ok(VrmModel {
        file: file.to_string(),
        name: Path::new(file)
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or(file)
            .to_string(),
        size: bytes.len() as u64,
    })
}

/// Remove a model from the library. `Ok(false)` means it wasn't there.
pub fn delete_vrm(data_dir: &Path, file: &str) -> anyhow::Result<bool> {
    validate_vrm_file_name(file)?;
    let path = vrm_path(data_dir, file);
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path).map_err(|e| anyhow::anyhow!("failed to delete vrm {file}: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_traversal_and_separators() {
        for bad in [
            "../evil.vrm",
            "a/b.vrm",
            "a\\b.vrm",
            "C:model.vrm",
            "",
            ".hidden.vrm",
        ] {
            assert!(validate_vrm_file_name(bad).is_err(), "expected {bad:?} to be rejected");
        }
    }

    /// An invalid name is a caller mistake, not "no such model" — callers
    /// map the two to different HTTP statuses.
    #[test]
    fn vrm_exists_errors_on_an_invalid_name_rather_than_answering_false() {
        let dir = temp_data_dir();
        assert!(vrm_exists(&dir, "../escape.vrm").is_err());
        assert!(!vrm_exists(&dir, "absent.vrm").unwrap());
    }

    #[test]
    fn validate_rejects_non_vrm_extension() {
        assert!(validate_vrm_file_name("model.png").is_err());
        assert!(validate_vrm_file_name("model").is_err());
    }

    #[test]
    fn validate_accepts_plain_names_case_insensitively() {
        assert!(validate_vrm_file_name("model.vrm").is_ok());
        assert!(validate_vrm_file_name("Model.VRM").is_ok());
        assert!(validate_vrm_file_name("日本語のモデル.vrm").is_ok());
    }

    fn temp_data_dir() -> PathBuf {
        std::env::temp_dir().join(format!("npc-core-vrm-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn list_is_empty_when_folder_is_absent() {
        let dir = temp_data_dir();
        assert!(list_vrm_models(&dir).unwrap().is_empty());
    }

    #[test]
    fn save_list_read_delete_round_trip() {
        let dir = temp_data_dir();
        let saved = save_vrm(&dir, "alice.vrm", b"fake-bytes").unwrap();
        assert_eq!(saved.name, "alice");
        assert_eq!(saved.size, 10);

        let listed = list_vrm_models(&dir).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].file, "alice.vrm");

        assert_eq!(read_vrm(&dir, "alice.vrm").unwrap().unwrap(), b"fake-bytes");
        assert!(vrm_exists(&dir, "alice.vrm").unwrap());
        assert!(delete_vrm(&dir, "alice.vrm").unwrap());
        assert!(!vrm_exists(&dir, "alice.vrm").unwrap());
        assert!(read_vrm(&dir, "alice.vrm").unwrap().is_none());
        assert!(!delete_vrm(&dir, "alice.vrm").unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An invalid name is a caller mistake, not "no such model" — same split
    /// as [`vrm_exists`]'s own test above.
    #[test]
    fn vrm_file_stat_errors_on_an_invalid_name_rather_than_answering_missing() {
        let dir = temp_data_dir();
        assert!(vrm_file_stat(&dir, "../escape.vrm").is_err());
    }

    #[test]
    fn vrm_file_stat_is_none_for_a_model_that_is_not_in_the_library() {
        let dir = temp_data_dir();
        assert!(vrm_file_stat(&dir, "absent.vrm").unwrap().is_none());
    }

    #[test]
    fn vrm_file_stat_reports_size_and_a_recent_modified_time() {
        let dir = temp_data_dir();
        let before = std::time::SystemTime::now();
        save_vrm(&dir, "alice.vrm", b"fake-bytes").unwrap();
        let after = std::time::SystemTime::now();

        let stat = vrm_file_stat(&dir, "alice.vrm").unwrap().unwrap();
        assert_eq!(stat.size, 10);
        // Loose bounds (rather than exact equality) tolerate coarser
        // filesystem timestamp resolution and any clock skew between the
        // two `SystemTime::now()` calls above and the write in between them.
        let tolerance = std::time::Duration::from_secs(2);
        assert!(stat.modified + tolerance >= before, "modified {:?} should not be far before {before:?}", stat.modified);
        assert!(stat.modified <= after + tolerance, "modified {:?} should not be far after {after:?}", stat.modified);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Anything the operator drops into the folder by hand is part of the
    /// library; non-`.vrm` files sharing it are not.
    #[test]
    fn list_ignores_non_vrm_files() {
        let dir = temp_data_dir();
        std::fs::create_dir_all(vrm_dir(&dir)).unwrap();
        std::fs::write(vrm_dir(&dir).join("notes.txt"), b"x").unwrap();
        save_vrm(&dir, "bob.VRM", b"x").unwrap();

        let listed = list_vrm_models(&dir).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].file, "bob.VRM");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
