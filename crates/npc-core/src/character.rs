//! Character sheets: the persona data npc-talk turns into a system prompt.
//!
//! Characters can be authored by hand (JSON files under
//! `{data_dir}/characters/`) or imported from a tc-town export bundle via
//! [`import_tc_town_export`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;

/// The persona fields that make up a character. All free text; any field may
/// be empty.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CharacterSheet {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub persona: String,
    #[serde(default)]
    pub speech_style: String,
    #[serde(default)]
    pub likes: String,
    #[serde(default)]
    pub relationships: String,
    #[serde(default)]
    pub notes: String,
}

/// The 3D model a character is displayed as. Only VRM is supported, but the
/// `kind` tag is kept on the wire so an added avatar type later doesn't have
/// to break the shape every stored character file already has.
///
/// `file` names a `.vrm` in the local model folder (`{data_dir}/vrm/`, see
/// [`crate::vrm`]) rather than embedding the model: the folder is the
/// library, so a character just points at one of its entries. A dangling
/// pointer — the character names a model the folder no longer has — is not
/// an error anywhere; the UI falls back to the plain initial avatar, which
/// is also what a character with no avatar at all gets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Avatar {
    /// Always `"vrm"` today.
    pub kind: String,
    /// File name (with extension) inside `{data_dir}/vrm/`.
    pub file: String,
}

impl Avatar {
    /// A VRM avatar pointing at `file` in the model folder.
    pub fn vrm(file: impl Into<String>) -> Self {
        Avatar {
            kind: "vrm".to_string(),
            file: file.into(),
        }
    }
}

/// A stored character: identity/timestamps plus the sheet, optional voice
/// selection, and optional avatar model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Character {
    pub id: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub sheet: CharacterSheet,
    #[serde(default)]
    pub voice_model: Option<String>,
    #[serde(default)]
    pub voice_name: Option<String>,
    /// The VRM this character is displayed as, if one has been assigned.
    /// Absent on every character saved before avatars existed, hence
    /// `#[serde(default)]` rather than a required field.
    #[serde(default)]
    pub avatar: Option<Avatar>,
}

// ---------------------------------------------------------------------
// tc-town export import
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TcTownExport {
    #[serde(default)]
    #[allow(dead_code)]
    app: String,
    #[serde(default)]
    #[allow(dead_code)]
    version: u64,
    #[serde(default)]
    #[allow(dead_code)]
    kind: String,
    #[serde(default)]
    characters: Vec<TcTownCharacter>,
}

#[derive(Debug, Deserialize)]
struct TcTownCharacter {
    #[serde(default)]
    id: String,
    #[serde(default, rename = "createdAt")]
    created_at: String,
    #[serde(default, rename = "updatedAt")]
    updated_at: String,
    #[serde(default)]
    sheet: TcTownSheet,
    #[serde(default, rename = "voiceModel")]
    voice_model: Option<String>,
    #[serde(default, rename = "voiceName")]
    voice_name: Option<String>,
    #[serde(default)]
    avatar: Option<TcTownAvatar>,
}

/// tc-town's avatar reference. It points into tc-town's *browser* model
/// library (IndexedDB, keyed by `blobKey`/`checksum`), which this process
/// has no access to — but the original `fileName` is enough to link the
/// character to a model of the same name in the local `{data_dir}/vrm/`
/// folder, so dropping the same `.vrm` in there makes an imported character
/// show up with its avatar already attached. Nothing breaks if the file
/// isn't there: a dangling avatar renders as the plain initial (see
/// [`Avatar`]).
///
/// Image avatars (`kind: "image"`) carry no usable file name and are
/// dropped.
#[derive(Debug, Deserialize)]
struct TcTownAvatar {
    #[serde(default)]
    kind: String,
    #[serde(default, rename = "fileName")]
    file_name: String,
}

/// Map a tc-town avatar onto a local one, keeping only VRM references that
/// actually name a plausible model file.
fn avatar_from_tc_town(avatar: Option<TcTownAvatar>) -> Option<Avatar> {
    let avatar = avatar?;
    if avatar.kind != "vrm" {
        return None;
    }
    let file = avatar.file_name.trim();
    // The name reaches the file system through `crate::vrm`, so refuse
    // anything that isn't a plain `*.vrm` name here rather than storing a
    // pointer that every later lookup would have to reject anyway.
    crate::vrm::validate_vrm_file_name(file).ok()?;
    Some(Avatar::vrm(file))
}

#[derive(Debug, Default, Deserialize)]
struct TcTownSheet {
    #[serde(default)]
    name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    persona: String,
    #[serde(default, rename = "speechStyle")]
    speech_style: String,
    #[serde(default)]
    likes: String,
    #[serde(default)]
    relationships: String,
    #[serde(default)]
    notes: String,
}

/// Parse a tc-town character export bundle
/// (`{"app":"tc-town","version":1,"kind":"character","characters":[...]}`)
/// into a list of [`Character`]s. Defensive against missing fields — every
/// field defaults to an empty string/None rather than erroring.
pub fn import_tc_town_export(json: &str) -> anyhow::Result<Vec<Character>> {
    let export: TcTownExport = serde_json::from_str(json)
        .map_err(|e| anyhow::anyhow!("failed to parse tc-town export: {e}"))?;

    Ok(export
        .characters
        .into_iter()
        .map(|c| Character {
            id: c.id,
            created_at: c.created_at,
            updated_at: c.updated_at,
            sheet: CharacterSheet {
                name: c.sheet.name,
                summary: c.sheet.summary,
                persona: c.sheet.persona,
                speech_style: c.sheet.speech_style,
                likes: c.sheet.likes,
                relationships: c.sheet.relationships,
                notes: c.sheet.notes,
            },
            voice_model: c.voice_model,
            voice_name: c.voice_name,
            avatar: avatar_from_tc_town(c.avatar),
        })
        .collect())
}

// ---------------------------------------------------------------------
// persona prompt
// ---------------------------------------------------------------------

/// Render a character sheet into the Japanese sectioned persona prompt used
/// to instruct the LLM to role-play as this character. Empty sections are
/// skipped.
pub fn persona_prompt(sheet: &CharacterSheet) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "あなたは「{}」というキャラクターです。以下の設定に従って、そのキャラクター本人として応答してください。\n",
        sheet.name
    ));

    let sections: [(&str, &str); 6] = [
        ("# 概要", &sheet.summary),
        ("# 人物・背景", &sheet.persona),
        ("# 話し方", &sheet.speech_style),
        ("# 好きなもの・嫌いなもの", &sheet.likes),
        ("# 人間関係", &sheet.relationships),
        ("# メモ", &sheet.notes),
    ];

    for (heading, body) in sections {
        if body.trim().is_empty() {
            continue;
        }
        out.push('\n');
        out.push_str(heading);
        out.push('\n');
        out.push_str(body.trim());
        out.push('\n');
    }

    out
}

// ---------------------------------------------------------------------
// character store
// ---------------------------------------------------------------------

fn characters_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("characters")
}

fn character_path(data_dir: &Path, id: &str) -> PathBuf {
    characters_dir(data_dir).join(format!("{id}.json"))
}

/// Refuse an id that would escape `{data_dir}/characters/`.
///
/// Ids reach [`load_character`] / [`save_character`] straight from an HTTP
/// path (`/api/characters/:id/avatar`), and a save with `../../x` in it
/// would write a JSON file anywhere the process can reach. Real ids are
/// UUIDs (tc-town's `crypto.randomUUID`), but hand-authored characters may
/// use anything readable — so this rejects the dangerous shapes rather than
/// allowlisting a character set that could exclude a legitimate existing
/// file. Both separators are checked, not just the platform's own.
fn validate_character_id(id: &str) -> anyhow::Result<()> {
    let invalid = id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id.contains(':')
        || id.starts_with('.');
    if invalid {
        anyhow::bail!("invalid character id: {id:?}");
    }
    Ok(())
}

/// List all characters stored under `{data_dir}/characters/`. Missing
/// directory yields an empty list rather than an error.
pub fn list_characters(data_dir: &Path) -> anyhow::Result<Vec<Character>> {
    let dir = characters_dir(data_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let data = std::fs::read_to_string(&path)?;
        match serde_json::from_str::<Character>(&data) {
            Ok(c) => out.push(c),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping unreadable character file");
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Save (create or overwrite) a character under `{data_dir}/characters/`,
/// creating the directory if needed.
pub fn save_character(data_dir: &Path, character: &Character) -> anyhow::Result<()> {
    validate_character_id(&character.id)?;
    let dir = characters_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let path = character_path(data_dir, &character.id);
    let data = serde_json::to_string_pretty(character)?;
    std::fs::write(path, data)?;
    Ok(())
}

/// Load a single character by id.
pub fn load_character(data_dir: &Path, id: &str) -> anyhow::Result<Character> {
    validate_character_id(id)?;
    let path = character_path(data_dir, id);
    let data = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("failed to read character {id}: {e}"))?;
    let character = serde_json::from_str(&data)
        .map_err(|e| anyhow::anyhow!("failed to parse character {id}: {e}"))?;
    Ok(character)
}

/// Load the currently active character (per `config.character.active_id`),
/// if any is set and it exists on disk.
pub fn active_character(data_dir: &Path, config: &Config) -> anyhow::Result<Option<Character>> {
    if config.character.active_id.is_empty() {
        return Ok(None);
    }
    match load_character(data_dir, &config.character.active_id) {
        Ok(c) => Ok(Some(c)),
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_minimal_export() {
        let json = r#"{
            "app": "tc-town",
            "version": 1,
            "kind": "character",
            "characters": [
                {
                    "id": "abc123",
                    "createdAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-02T00:00:00Z",
                    "sheet": { "name": "テスト" },
                    "avatar": { "kind": "image", "blobKey": "avatar-img-1" }
                }
            ]
        }"#;
        let chars = import_tc_town_export(json).unwrap();
        assert_eq!(chars.len(), 1);
        assert_eq!(chars[0].id, "abc123");
        assert_eq!(chars[0].sheet.name, "テスト");
        assert_eq!(chars[0].sheet.summary, "");
        // An image avatar has no local counterpart, so it's dropped.
        assert!(chars[0].avatar.is_none());
    }

    #[test]
    fn imports_vrm_avatar_as_a_local_model_reference() {
        let json = r#"{
            "characters": [
                {
                    "id": "abc123",
                    "sheet": { "name": "テスト" },
                    "avatar": {
                        "kind": "vrm",
                        "blobKey": "file-xyz",
                        "checksum": "deadbeef",
                        "fileName": "alice.vrm"
                    }
                }
            ]
        }"#;
        let avatar = import_tc_town_export(json).unwrap()[0].avatar.clone().unwrap();
        assert_eq!(avatar.kind, "vrm");
        assert_eq!(avatar.file, "alice.vrm");
    }

    /// The name lands in a file-system path, so a crafted export must not be
    /// able to point a character outside the model folder.
    #[test]
    fn drops_vrm_avatar_with_an_unsafe_file_name() {
        let json = r#"{
            "characters": [
                {
                    "id": "abc123",
                    "sheet": { "name": "テスト" },
                    "avatar": { "kind": "vrm", "fileName": "../../secrets.vrm" }
                }
            ]
        }"#;
        assert!(import_tc_town_export(json).unwrap()[0].avatar.is_none());
    }

    #[test]
    fn imports_character_without_avatar_field() {
        let json = r#"{"characters":[{"id":"abc","sheet":{"name":"テスト"}}]}"#;
        assert!(import_tc_town_export(json).unwrap()[0].avatar.is_none());
    }

    #[test]
    fn character_ids_that_would_escape_the_folder_are_rejected() {
        for bad in ["../evil", "a/b", "a\\b", "C:evil", "", ".hidden"] {
            assert!(validate_character_id(bad).is_err(), "expected {bad:?} to be rejected");
        }
        // A UUID (what tc-town generates) and a readable hand-authored id.
        assert!(validate_character_id("0f8fad5b-d9cb-469f-a165-70867728950e").is_ok());
        assert!(validate_character_id("みどり_01").is_ok());
    }

    /// The id reaches the file system through both entry points, so neither
    /// may act on one that escapes the folder.
    #[test]
    fn save_and_load_refuse_an_escaping_id() {
        let dir = std::env::temp_dir().join(format!("npc-core-character-test-{}", uuid::Uuid::new_v4()));
        let character = Character {
            id: "../escaped".to_string(),
            created_at: String::new(),
            updated_at: String::new(),
            sheet: CharacterSheet::default(),
            voice_model: None,
            voice_name: None,
            avatar: None,
        };
        assert!(save_character(&dir, &character).is_err());
        assert!(load_character(&dir, "../escaped").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persona_prompt_skips_empty_sections() {
        let sheet = CharacterSheet {
            name: "テスト".to_string(),
            summary: "概要テキスト".to_string(),
            ..Default::default()
        };
        let prompt = persona_prompt(&sheet);
        assert!(prompt.contains("あなたは「テスト」というキャラクターです"));
        assert!(prompt.contains("# 概要"));
        assert!(!prompt.contains("# 人物・背景"));
    }
}
