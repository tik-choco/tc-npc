//! Person records: memories tied to an individual rather than to a
//! conversation. Sibling of [`crate::character`]'s character-sheet store —
//! same on-disk shape (one JSON file per record under
//! `{data_dir}/people/`), same atomic tmp+rename save, same "missing
//! directory/file is not an error" loading semantics.
//!
//! A [`Person`] accumulates over time: npc-vision reports sightings
//! (`bus::msg::PERSON_SEEN`), chat can mention who's speaking, and an
//! operator can hand-edit `notes`/`aliases` from the web UI. `person_to_wire`
//! is the single place that turns the on-disk snake_case shape into the
//! camelCase `PersonRecord` the web UI and REST API speak.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A person record, one file per person under `{data_dir}/people/{id}.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub first_seen: i64,
    #[serde(default)]
    pub last_seen: i64,
    #[serde(default)]
    pub encounter_count: u32,
    #[serde(default)]
    pub appearance: String,
    #[serde(default)]
    pub facts: Vec<PersonFact>,
    #[serde(default)]
    pub familiarity: f32,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub notes: String,
}

/// A single fact learned about a [`Person`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonFact {
    pub text: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub created_at: i64,
}

// ---------------------------------------------------------------------
// construction / matching
// ---------------------------------------------------------------------

/// Build a brand-new record: fresh uuid v4 id, `first_seen`/`last_seen` set
/// to now, `encounter_count` starting at 1 (this call *is* the first
/// encounter).
pub fn new_person(name: &str, source: &str) -> Person {
    let now = chrono::Utc::now().timestamp();
    Person {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        aliases: Vec::new(),
        first_seen: now,
        last_seen: now,
        encounter_count: 1,
        appearance: String::new(),
        facts: Vec::new(),
        familiarity: 0.0,
        source: source.to_string(),
        notes: String::new(),
    }
}

/// Honorifics stripped from the end of a name by [`normalize_person_name`],
/// longest/most-specific first so e.g. "せんせい" doesn't get shadowed by a
/// shorter unrelated suffix.
const HONORIFICS: [&str; 7] = ["せんせい", "先生", "さん", "くん", "ちゃん", "様", "氏"];

/// Normalize a name for matching: trim surrounding whitespace, fold
/// full-width spaces to half-width, strip a single trailing honorific, then
/// lowercase. Operates on whole `char`s throughout (via `str` methods like
/// `strip_suffix`, never byte slicing) so multi-byte Japanese text is never
/// split mid-character.
pub fn normalize_person_name(name: &str) -> String {
    let folded = name.trim().replace('\u{3000}', " ");
    let mut trimmed = folded.trim();

    for honorific in HONORIFICS {
        if let Some(stripped) = trimmed.strip_suffix(honorific) {
            trimmed = stripped;
            break;
        }
    }

    trimmed.trim().to_lowercase()
}

/// Find the person in `people` whose `name` or any `aliases` entry matches
/// `name` under [`normalize_person_name`]. An empty normalized `name` never
/// matches (avoids every unnamed person matching each other).
pub fn find_person<'a>(people: &'a [Person], name: &str) -> Option<&'a Person> {
    let target = normalize_person_name(name);
    if target.is_empty() {
        return None;
    }
    people.iter().find(|p| {
        normalize_person_name(&p.name) == target
            || p.aliases.iter().any(|a| normalize_person_name(a) == target)
    })
}

// ---------------------------------------------------------------------
// wire format
// ---------------------------------------------------------------------

/// Convert a [`Person`] into the camelCase `PersonRecord` JSON shape shared
/// with the web UI (see the person-memory contract §5). `Person`'s own serde
/// derive stays snake_case — that's the on-disk format — so this is the one
/// place the two shapes meet.
pub fn person_to_wire(p: &Person) -> serde_json::Value {
    serde_json::json!({
        "id": p.id,
        "name": p.name,
        "aliases": p.aliases,
        "firstSeen": p.first_seen,
        "lastSeen": p.last_seen,
        "encounterCount": p.encounter_count,
        "appearance": p.appearance,
        "facts": p.facts.iter().map(person_fact_to_wire).collect::<Vec<_>>(),
        "familiarity": p.familiarity,
        "source": p.source,
        "notes": p.notes,
    })
}

fn person_fact_to_wire(f: &PersonFact) -> serde_json::Value {
    serde_json::json!({
        "text": f.text,
        "source": f.source,
        "createdAt": f.created_at,
    })
}

// ---------------------------------------------------------------------
// person store
// ---------------------------------------------------------------------

/// `{data_dir}/people`.
pub fn people_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("people")
}

fn person_path(data_dir: &Path, id: &str) -> PathBuf {
    people_dir(data_dir).join(format!("{id}.json"))
}

/// Reject any id that isn't safe to use as a filename component, so a
/// crafted id (e.g. `../../etc/passwd`) can never escape `{data_dir}/people/`.
fn validate_person_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        anyhow::bail!("invalid person id: {id:?}");
    }
    Ok(())
}

/// List all people stored under `{data_dir}/people/`. A missing directory
/// yields an empty list rather than an error. A file that fails to parse is
/// logged and skipped so one corrupt record doesn't take down the rest.
pub fn list_people(data_dir: &Path) -> anyhow::Result<Vec<Person>> {
    let dir = people_dir(data_dir);
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
        match serde_json::from_str::<Person>(&data) {
            Ok(p) => out.push(p),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping unreadable person file");
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Load a single person by id, if it exists. Unlike [`crate::character::load_character`]
/// this returns `Ok(None)` (not an error) for a missing file, matching the
/// person-memory contract's `Option`-returning signature.
pub fn load_person(data_dir: &Path, id: &str) -> anyhow::Result<Option<Person>> {
    validate_person_id(id)?;
    let path = person_path(data_dir, id);
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("failed to read person {id}: {e}"))?;
    let person = serde_json::from_str(&data)
        .map_err(|e| anyhow::anyhow!("failed to parse person {id}: {e}"))?;
    Ok(Some(person))
}

/// Save (create or overwrite) a person under `{data_dir}/people/`, creating
/// the directory if needed. Writes to a temp file in the same directory then
/// renames over the target path, which is atomic on both POSIX and Windows.
pub fn save_person(data_dir: &Path, person: &Person) -> anyhow::Result<()> {
    validate_person_id(&person.id)?;
    let dir = people_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let path = person_path(data_dir, &person.id);
    let tmp_path = tmp_path_for(&path);
    let data = serde_json::to_string_pretty(person)?;
    std::fs::write(&tmp_path, data)?;
    std::fs::rename(&tmp_path, &path)?;
    Ok(())
}

/// Delete a person by id. Returns `true` if a file was removed, `false` if
/// there was nothing to delete.
pub fn delete_person(data_dir: &Path, id: &str) -> anyhow::Result<bool> {
    validate_person_id(id)?;
    let path = person_path(data_dir, id);
    if !path.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&path)?;
    Ok(true)
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "person.json".to_string());
    name.push_str(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_data_dir() -> PathBuf {
        std::env::temp_dir().join(format!("npc-core-person-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = temp_data_dir();
        let mut person = new_person("太郎", "manual");
        person.aliases.push("たろちゃん".to_string());
        person.facts.push(PersonFact {
            text: "犬を飼っている".to_string(),
            source: "chat".to_string(),
            created_at: 12345,
        });

        save_person(&dir, &person).unwrap();
        let loaded = load_person(&dir, &person.id).unwrap().unwrap();
        assert_eq!(loaded.id, person.id);
        assert_eq!(loaded.name, "太郎");
        assert_eq!(loaded.aliases, vec!["たろちゃん".to_string()]);
        assert_eq!(loaded.facts.len(), 1);
        assert_eq!(loaded.facts[0].text, "犬を飼っている");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_person_returns_none() {
        let dir = temp_data_dir();
        let result = load_person(&dir, "does-not-exist").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn list_people_on_missing_dir_is_empty() {
        let dir = temp_data_dir();
        let people = list_people(&dir).unwrap();
        assert!(people.is_empty());
    }

    #[test]
    fn list_people_skips_corrupt_files_and_keeps_the_rest() {
        let dir = temp_data_dir();
        let person = new_person("花子", "manual");
        save_person(&dir, &person).unwrap();

        // Drop a corrupt JSON file alongside the valid one.
        std::fs::write(people_dir(&dir).join("broken.json"), "{ not json").unwrap();

        let people = list_people(&dir).unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].id, person.id);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_person_removes_file_and_reports_existence() {
        let dir = temp_data_dir();
        let person = new_person("次郎", "manual");
        save_person(&dir, &person).unwrap();

        assert!(delete_person(&dir, &person.id).unwrap());
        assert!(load_person(&dir, &person.id).unwrap().is_none());
        // Second delete: nothing left to remove.
        assert!(!delete_person(&dir, &person.id).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_invalid_ids_to_prevent_path_traversal() {
        let dir = temp_data_dir();
        assert!(load_person(&dir, "../../etc/passwd").is_err());
        assert!(load_person(&dir, "").is_err());
        assert!(delete_person(&dir, "some/slash").is_err());

        let mut person = new_person("test", "manual");
        person.id = "bad id with spaces".to_string();
        assert!(save_person(&dir, &person).is_err());
    }

    #[test]
    fn normalize_person_name_strips_honorifics_and_folds_case() {
        assert_eq!(normalize_person_name("太郎さん"), "太郎");
        assert_eq!(normalize_person_name("花子ちゃん"), "花子");
        assert_eq!(normalize_person_name("次郎くん"), "次郎");
        assert_eq!(normalize_person_name("鈴木様"), "鈴木");
        assert_eq!(normalize_person_name("田中氏"), "田中");
        assert_eq!(normalize_person_name("山田先生"), "山田");
        assert_eq!(normalize_person_name("鈴木せんせい"), "鈴木");
        // Full-width space folded, then trimmed.
        assert_eq!(normalize_person_name("　太郎　"), "太郎");
        // ASCII names lowercase.
        assert_eq!(normalize_person_name("  Alice  "), "alice");
        // No honorific: unchanged (besides trim/lowercase).
        assert_eq!(normalize_person_name("太郎"), "太郎");
    }

    #[test]
    fn find_person_matches_by_name_or_alias() {
        let mut a = new_person("太郎", "manual");
        a.aliases.push("たろ".to_string());
        let b = new_person("花子", "manual");
        let people = vec![a.clone(), b];

        let found = find_person(&people, "太郎さん").unwrap();
        assert_eq!(found.id, a.id);

        let found_alias = find_person(&people, "たろ").unwrap();
        assert_eq!(found_alias.id, a.id);

        assert!(find_person(&people, "見知らぬ人").is_none());
        assert!(find_person(&people, "").is_none());
    }

    #[test]
    fn person_to_wire_uses_camel_case_keys() {
        let mut person = new_person("太郎", "chat");
        person.facts.push(PersonFact {
            text: "コーヒーが好き".to_string(),
            source: "chat".to_string(),
            created_at: 999,
        });

        let wire = person_to_wire(&person);
        assert_eq!(wire["id"], person.id);
        assert_eq!(wire["name"], "太郎");
        assert_eq!(wire["firstSeen"], person.first_seen);
        assert_eq!(wire["lastSeen"], person.last_seen);
        assert_eq!(wire["encounterCount"], 1);
        assert_eq!(wire["familiarity"], 0.0);
        assert_eq!(wire["facts"][0]["text"], "コーヒーが好き");
        assert_eq!(wire["facts"][0]["createdAt"], 999);
        // snake_case keys must not leak through.
        assert!(wire.get("first_seen").is_none());
        assert!(wire.get("encounter_count").is_none());
        assert!(wire["facts"][0].get("created_at").is_none());
    }
}
