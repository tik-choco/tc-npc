//! tc-town character catalog discovery: parses `CatalogEntryWire`
//! broadcasts from the public `tc-town-character-catalog-v1` room (see
//! `tc-town/src/lib/catalogTypes.ts` and `catalog.ts`'s `connectCatalog`/
//! `handleEntryWire`), fetches the referenced `CatalogPayloadV1` payload via
//! mist content storage, converts the embedded character into an
//! `npc_core::Character`, and stores it under `{data_dir}/catalog/{id}.json`
//! -- deliberately NOT `{data_dir}/characters/` (which
//! `npc_core::character`'s store reads for the active persona): these are
//! *discovered* characters an operator hasn't chosen to import yet.

use std::path::Path;

use npc_core::{Bus, Character, CharacterSheet};
use serde::Deserialize;
use serde_json::Value;

/// The public catalog room id tc-town's web client joins (see
/// `tc-town/src/lib/catalogTypes.ts`'s `CATALOG_ROOM_ID`).
pub const CATALOG_ROOM_ID: &str = "tc-town-character-catalog-v1";

/// The subset of tc-town's `ExportedCharacter` (`tc-town/src/lib/exportImport.ts`,
/// `tc-town/src/types.ts`'s `Character`) this module cares about -- the same
/// shape `npc_core::character::import_tc_town_export` already parses for a
/// bulk export bundle. `avatar`/`llmProfileId`/`worldId`/any other extra
/// fields are ignored automatically (no `deny_unknown_fields`), matching
/// this task's "ignore avatar/vrm" instruction.
#[derive(Debug, Default, Deserialize)]
struct CatalogCharacter {
    #[serde(default)]
    id: String,
    #[serde(default, rename = "createdAt")]
    created_at: String,
    #[serde(default, rename = "updatedAt")]
    updated_at: String,
    #[serde(default)]
    sheet: CatalogSheet,
    #[serde(default, rename = "voiceModel")]
    voice_model: Option<String>,
    #[serde(default, rename = "voiceName")]
    voice_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct CatalogSheet {
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

/// Handles one raw event received while listening to the catalog room:
/// parses it as a `CatalogEntryWire`, fetches + defensively validates the
/// `CatalogPayloadV1` JSON its `cid` points to, converts the embedded
/// character into an `npc_core::Character`, and saves it under
/// `{data_dir}/catalog/`. Anything that doesn't parse as a well-formed
/// catalog entry is silently ignored -- the same "coexist on the wire by
/// shape" approach `mistl` uses for its own room-multiplexed protocols
/// (each handler parses inbound bytes against its own schema and ignores
/// what it can't parse), which matters here since a custom
/// `config.mist.room_id` and the catalog room are mutually exclusive but
/// this handler only ever runs when the catalog room is the one joined.
///
/// NOTE (v1): unlike tc-town's own web client (`catalog.ts`'s
/// `handleEntryWire`, which calls `verifyWire` against the entry's did:key
/// `signature`), this deliberately does NOT verify
/// `CatalogEntryWire.signature` before trusting an entry -- there's no DID
/// identity/crypto story on the tc-npc side yet. A forged/unsigned entry
/// could at worst make tc-npc fetch and store a bogus character sheet under
/// `catalog/`, never `characters/`, so it can never silently become the
/// active persona (that still requires an operator to import it).
pub async fn handle_raw_event(data: &[u8], data_dir: &Path, bus: &Bus) {
    let Ok(value) = serde_json::from_slice::<Value>(data) else {
        return;
    };
    if value.get("type").and_then(Value::as_str) != Some("tc-town:catalog-entry") {
        return;
    }
    let Some(cid) = value.get("cid").and_then(Value::as_str).map(str::to_string) else {
        tracing::debug!("mist: catalog entry wire missing `cid`, ignoring");
        return;
    };

    let bytes = match crate::engine::storage_get(&cid).await {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(cid = %cid, error = %err, "mist: failed to fetch catalog payload");
            return;
        }
    };

    let payload: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(cid = %cid, error = %err, "mist: catalog payload is not valid JSON, ignoring");
            return;
        }
    };

    if payload.get("app").and_then(Value::as_str) != Some("tc-town")
        || payload.get("kind").and_then(Value::as_str) != Some("catalog-character")
        || payload.get("version").and_then(Value::as_u64) != Some(1)
    {
        tracing::warn!(cid = %cid, "mist: catalog payload has an unexpected shape, ignoring");
        return;
    }

    let character: CatalogCharacter = match payload.get("character").cloned() {
        Some(v) => match serde_json::from_value(v) {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!(cid = %cid, error = %err, "mist: failed to parse catalog character, ignoring");
                return;
            }
        },
        None => {
            tracing::warn!(cid = %cid, "mist: catalog payload missing `character`, ignoring");
            return;
        }
    };

    if character.id.is_empty() {
        tracing::warn!(cid = %cid, "mist: catalog character has no id, ignoring");
        return;
    }

    let npc_character = Character {
        id: character.id.clone(),
        created_at: character.created_at,
        updated_at: character.updated_at,
        sheet: CharacterSheet {
            name: character.sheet.name,
            summary: character.sheet.summary,
            persona: character.sheet.persona,
            speech_style: character.sheet.speech_style,
            likes: character.sheet.likes,
            relationships: character.sheet.relationships,
            notes: character.sheet.notes,
        },
        voice_model: character.voice_model,
        voice_name: character.voice_name,
    };

    if let Err(err) = save_discovered(data_dir, &npc_character) {
        tracing::warn!(id = %npc_character.id, error = %err, "mist: failed to save discovered catalog character");
        return;
    }

    let display_name = if npc_character.sheet.name.trim().is_empty() {
        "(名称未設定)".to_string()
    } else {
        npc_character.sheet.name.clone()
    };
    tracing::info!(id = %npc_character.id, name = %display_name, "mist: catalog character discovered");
    bus.publish(
        "npc:ui",
        "action_log",
        serde_json::json!({ "text": format!("mist: キャラクター「{display_name}」を発見 (catalog)") }),
    );
}

/// Saves under `{data_dir}/catalog/{id}.json`, creating the directory if
/// needed. A deliberately separate store from
/// `npc_core::character::save_character` (`{data_dir}/characters/`) -- see
/// this module's doc comment.
fn save_discovered(data_dir: &Path, character: &Character) -> anyhow::Result<()> {
    let dir = data_dir.join("catalog");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", character.id));
    let data = serde_json::to_string_pretty(character)?;
    std::fs::write(path, data)?;
    Ok(())
}
