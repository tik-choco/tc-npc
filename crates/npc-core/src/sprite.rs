//! 2D sprite-sheet library: the `.png` sheets sitting in `{data_dir}/sprites/`.
//!
//! The second display mode, alongside [`crate::vrm`]. A character that has
//! no VRM — or a machine that can't render one — still gets a body: a single
//! PNG sheet of animation frames, drawn by shifting a background offset
//! rather than by running a 3D scene.
//!
//! Deliberately the same shape as the VRM library: a plain local folder,
//! entries identified by **file name**, anything dropped in by hand picked
//! up on the next listing, and every name validated before it reaches the
//! filesystem (these arrive from HTTP paths, where a `..` would otherwise
//! read or delete a file anywhere on disk). The two libraries are separate
//! folders because they hold different things, not because they behave
//! differently.
//!
//! **Frame geometry is not stored here.** A sheet is just a PNG; how many
//! rows and columns it is cut into is the renderer's business, and baking a
//! grid into the file's identity would mean a sheet could be "wrong" on
//! disk. The web UI owns that constant.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The extension (lowercased) a file must have to be treated as a sheet.
const SPRITE_EXTENSION: &str = "png";

/// One sprite sheet in the library. Bytes are not included — listing must
/// stay cheap; [`read_sprite`] fetches those for the sheet being displayed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpriteSheet {
    /// File name including the `.png` extension — the sheet's identity.
    pub file: String,
    /// File name without the extension, for display.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
}

/// `{data_dir}/sprites` — the library folder.
pub fn sprite_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("sprites")
}

/// Reject anything that isn't a plain `*.png` file name. Same rules as
/// [`crate::vrm::validate_vrm_file_name`], for the same reason: both
/// separators (so a name crafted on one OS can't slip through on another), a
/// Windows drive prefix, `..` anywhere rather than only as a whole
/// component, and a leading dot.
pub fn validate_sprite_file_name(file: &str) -> anyhow::Result<()> {
    let invalid = file.is_empty()
        || file.contains('/')
        || file.contains('\\')
        || file.contains("..")
        || file.contains(':')
        || file.starts_with('.');
    if invalid {
        anyhow::bail!("invalid sprite file name: {file:?}");
    }
    if !has_sprite_extension(Path::new(file)) {
        anyhow::bail!("not a .png file: {file:?}");
    }
    Ok(())
}

/// Case-insensitive `.png` extension test — exporters routinely emit `.PNG`.
fn has_sprite_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(SPRITE_EXTENSION))
}

fn sprite_path(data_dir: &Path, file: &str) -> PathBuf {
    sprite_dir(data_dir).join(file)
}

/// Every sheet in the library folder, sorted by name. A missing folder
/// yields an empty list rather than an error — an install that has never had
/// a sheet added is a normal state, not a fault.
pub fn list_sprites(data_dir: &Path) -> anyhow::Result<Vec<SpriteSheet>> {
    let dir = sprite_dir(data_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() || !has_sprite_extension(&path) {
            continue;
        }
        let Some(file) = path.file_name().and_then(|n| n.to_str()) else {
            // A name that isn't valid UTF-8 can't survive the JSON round
            // trip the web UI identifies sheets by, so it is skipped rather
            // than listed as something that could never be loaded.
            tracing::warn!(path = %path.display(), "skipping sprite with non-UTF-8 file name");
            continue;
        };
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.push(SpriteSheet {
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

/// Whether a sheet of this name is in the library. `Ok(false)` covers "not
/// there"; an invalid name is still `Err`, since that's a caller mistake
/// rather than an answer about the folder — callers map the two to
/// different HTTP statuses.
pub fn sprite_exists(data_dir: &Path, file: &str) -> anyhow::Result<bool> {
    validate_sprite_file_name(file)?;
    Ok(sprite_path(data_dir, file).is_file())
}

/// Raw PNG bytes for one sheet, or `Ok(None)` if no such file exists.
pub fn read_sprite(data_dir: &Path, file: &str) -> anyhow::Result<Option<Vec<u8>>> {
    validate_sprite_file_name(file)?;
    let path = sprite_path(data_dir, file);
    if !path.exists() {
        return Ok(None);
    }
    let bytes =
        std::fs::read(&path).map_err(|e| anyhow::anyhow!("failed to read sprite {file}: {e}"))?;
    Ok(Some(bytes))
}

/// Write `bytes` into the library as `file`, creating the folder if needed
/// and overwriting any sheet already under that name. Returns the entry.
pub fn save_sprite(data_dir: &Path, file: &str, bytes: &[u8]) -> anyhow::Result<SpriteSheet> {
    validate_sprite_file_name(file)?;
    let dir = sprite_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let path = sprite_path(data_dir, file);
    std::fs::write(&path, bytes)
        .map_err(|e| anyhow::anyhow!("failed to write sprite {file}: {e}"))?;
    Ok(SpriteSheet {
        file: file.to_string(),
        name: Path::new(file)
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or(file)
            .to_string(),
        size: bytes.len() as u64,
    })
}

/// Remove a sheet from the library. `Ok(false)` means it wasn't there.
pub fn delete_sprite(data_dir: &Path, file: &str) -> anyhow::Result<bool> {
    validate_sprite_file_name(file)?;
    let path = sprite_path(data_dir, file);
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path)
        .map_err(|e| anyhow::anyhow!("failed to delete sprite {file}: {e}"))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_data_dir() -> PathBuf {
        std::env::temp_dir().join(format!("npc-core-sprite-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn validate_rejects_traversal_and_separators() {
        for bad in [
            "../evil.png",
            "a/b.png",
            "a\\b.png",
            "C:sheet.png",
            "",
            ".hidden.png",
        ] {
            assert!(
                validate_sprite_file_name(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    /// A `.vrm` is not a sheet: the two libraries hold different things and
    /// neither accepts the other's files.
    #[test]
    fn validate_rejects_non_png_extension() {
        assert!(validate_sprite_file_name("model.vrm").is_err());
        assert!(validate_sprite_file_name("sheet").is_err());
    }

    #[test]
    fn validate_accepts_plain_names_case_insensitively() {
        assert!(validate_sprite_file_name("alice.png").is_ok());
        assert!(validate_sprite_file_name("Alice.PNG").is_ok());
        assert!(validate_sprite_file_name("日本語のシート.png").is_ok());
    }

    #[test]
    fn sprite_exists_errors_on_an_invalid_name_rather_than_answering_false() {
        let dir = temp_data_dir();
        assert!(sprite_exists(&dir, "../escape.png").is_err());
        assert!(!sprite_exists(&dir, "absent.png").unwrap());
    }

    #[test]
    fn list_is_empty_when_folder_is_absent() {
        let dir = temp_data_dir();
        assert!(list_sprites(&dir).unwrap().is_empty());
    }

    #[test]
    fn save_list_read_delete_round_trip() {
        let dir = temp_data_dir();
        let saved = save_sprite(&dir, "alice.png", b"fake-png").unwrap();
        assert_eq!(saved.name, "alice");
        assert_eq!(saved.size, 8);

        let listed = list_sprites(&dir).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].file, "alice.png");

        assert_eq!(read_sprite(&dir, "alice.png").unwrap().unwrap(), b"fake-png");
        assert!(sprite_exists(&dir, "alice.png").unwrap());
        assert!(delete_sprite(&dir, "alice.png").unwrap());
        assert!(!sprite_exists(&dir, "alice.png").unwrap());
        assert!(read_sprite(&dir, "alice.png").unwrap().is_none());
        assert!(!delete_sprite(&dir, "alice.png").unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Anything the operator drops into the folder by hand is part of the
    /// library; files of other types sharing it are not.
    #[test]
    fn list_ignores_non_png_files() {
        let dir = temp_data_dir();
        std::fs::create_dir_all(sprite_dir(&dir)).unwrap();
        std::fs::write(sprite_dir(&dir).join("notes.txt"), b"x").unwrap();
        std::fs::write(sprite_dir(&dir).join("model.vrm"), b"x").unwrap();
        save_sprite(&dir, "bob.PNG", b"x").unwrap();

        let listed = list_sprites(&dir).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].file, "bob.PNG");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The sprite and VRM folders are siblings, not the same directory —
    /// a sheet must never be listed by the model library or vice versa.
    #[test]
    fn sprite_dir_is_separate_from_the_vrm_dir() {
        let dir = temp_data_dir();
        assert_ne!(sprite_dir(&dir), crate::vrm::vrm_dir(&dir));
    }
}
