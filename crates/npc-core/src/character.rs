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

/// A stored character: identity/timestamps plus the sheet and optional voice
/// selection.
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
    // `avatar` (and any other extra fields) are intentionally ignored.
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
    let dir = characters_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let path = character_path(data_dir, &character.id);
    let data = serde_json::to_string_pretty(character)?;
    std::fs::write(path, data)?;
    Ok(())
}

/// Load a single character by id.
pub fn load_character(data_dir: &Path, id: &str) -> anyhow::Result<Character> {
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
                    "avatar": { "ignored": true }
                }
            ]
        }"#;
        let chars = import_tc_town_export(json).unwrap();
        assert_eq!(chars.len(), 1);
        assert_eq!(chars[0].id, "abc123");
        assert_eq!(chars[0].sheet.name, "テスト");
        assert_eq!(chars[0].sheet.summary, "");
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
