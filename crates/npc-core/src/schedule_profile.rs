//! Named, switchable schedule profiles: a saved `SchedulerConfig` snapshot
//! (one file per profile under `{data_dir}/schedule_profiles/`), so a user
//! can keep e.g. a "weekday" and a "event day" announcement set and flip
//! between them without hand-editing/re-importing JSON each time. Mirrors
//! `character.rs`'s storage pattern; unlike characters, "activating" a
//! profile also copies its `scheduler` content into the live
//! `config.scheduler` (done by the REST layer, not here — see
//! `crates/npc-server/src/rest.rs`'s `api_activate_schedule_profile`),
//! since `npc-scheduler` only ever reads `config.scheduler` directly rather
//! than resolving an active-profile pointer at fire time the way character
//! persona resolution does.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::SchedulerConfig;

/// A saved, named scheduler snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleProfile {
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
}

fn schedule_profiles_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("schedule_profiles")
}

fn schedule_profile_path(data_dir: &Path, id: &str) -> PathBuf {
    schedule_profiles_dir(data_dir).join(format!("{id}.json"))
}

/// Refuse an id that would escape `{data_dir}/schedule_profiles/` — same
/// guard as `character.rs`'s `validate_character_id`, needed because ids
/// reach these functions straight from an HTTP path segment.
fn validate_schedule_profile_id(id: &str) -> anyhow::Result<()> {
    let invalid = id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id.contains(':')
        || id.starts_with('.');
    if invalid {
        anyhow::bail!("invalid schedule profile id: {id:?}");
    }
    Ok(())
}

/// List all schedule profiles under `{data_dir}/schedule_profiles/`. Missing
/// directory yields an empty list, not an error.
pub fn list_schedule_profiles(data_dir: &Path) -> anyhow::Result<Vec<ScheduleProfile>> {
    let dir = schedule_profiles_dir(data_dir);
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
        match serde_json::from_str::<ScheduleProfile>(&data) {
            Ok(p) => out.push(p),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping unreadable schedule profile file");
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Save (create or overwrite) a schedule profile under
/// `{data_dir}/schedule_profiles/`, creating the directory if needed.
pub fn save_schedule_profile(data_dir: &Path, profile: &ScheduleProfile) -> anyhow::Result<()> {
    validate_schedule_profile_id(&profile.id)?;
    let dir = schedule_profiles_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let path = schedule_profile_path(data_dir, &profile.id);
    let data = serde_json::to_string_pretty(profile)?;
    std::fs::write(path, data)?;
    Ok(())
}

/// Load a single schedule profile by id.
pub fn load_schedule_profile(data_dir: &Path, id: &str) -> anyhow::Result<ScheduleProfile> {
    validate_schedule_profile_id(id)?;
    let path = schedule_profile_path(data_dir, id);
    let data = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("failed to read schedule profile {id}: {e}"))?;
    let profile = serde_json::from_str(&data)
        .map_err(|e| anyhow::anyhow!("failed to parse schedule profile {id}: {e}"))?;
    Ok(profile)
}

/// Delete a schedule profile by id. Not an error if it didn't exist.
pub fn delete_schedule_profile(data_dir: &Path, id: &str) -> anyhow::Result<()> {
    validate_schedule_profile_id(id)?;
    let path = schedule_profile_path(data_dir, id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AnnouncementConfig;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("npc-core-schedule-profile-{name}-{}", uuid::Uuid::new_v4()))
    }

    fn sample_profile(id: &str) -> ScheduleProfile {
        ScheduleProfile {
            id: id.to_string(),
            label: "Weekday".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            scheduler: SchedulerConfig {
                enabled: true,
                announcements: vec![AnnouncementConfig {
                    time: "09:00".to_string(),
                    text: "おはようございます".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = temp_dir("roundtrip");
        let profile = sample_profile("weekday");

        save_schedule_profile(&dir, &profile).unwrap();
        let loaded = load_schedule_profile(&dir, "weekday").unwrap();

        assert_eq!(loaded.id, "weekday");
        assert_eq!(loaded.label, "Weekday");
        assert_eq!(loaded.scheduler.announcements.len(), 1);
        assert_eq!(loaded.scheduler.announcements[0].text, "おはようございます");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_returns_saved_profiles_sorted_by_id() {
        let dir = temp_dir("list");
        save_schedule_profile(&dir, &sample_profile("weekday")).unwrap();
        save_schedule_profile(&dir, &sample_profile("event")).unwrap();
        save_schedule_profile(&dir, &sample_profile("holiday")).unwrap();

        let listed = list_schedule_profiles(&dir).unwrap();
        let ids: Vec<&str> = listed.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["event", "holiday", "weekday"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_on_missing_directory_returns_empty_not_error() {
        let dir = temp_dir("missing");
        let listed = list_schedule_profiles(&dir).unwrap();
        assert!(listed.is_empty());
    }

    #[test]
    fn ids_that_would_escape_the_folder_are_rejected() {
        for bad in ["../evil", "a/b", "a\\b", "C:evil", "", ".hidden"] {
            assert!(validate_schedule_profile_id(bad).is_err(), "expected {bad:?} to be rejected");
            let dir = temp_dir("escape");
            assert!(save_schedule_profile(&dir, &sample_profile(bad)).is_err());
            assert!(load_schedule_profile(&dir, bad).is_err());
            assert!(delete_schedule_profile(&dir, bad).is_err());
        }
    }

    #[test]
    fn delete_of_nonexistent_id_is_not_an_error() {
        let dir = temp_dir("delete-missing");
        assert!(delete_schedule_profile(&dir, "nope").is_ok());
    }

    #[test]
    fn delete_removes_a_saved_profile() {
        let dir = temp_dir("delete");
        save_schedule_profile(&dir, &sample_profile("weekday")).unwrap();
        assert!(load_schedule_profile(&dir, "weekday").is_ok());

        delete_schedule_profile(&dir, "weekday").unwrap();
        assert!(load_schedule_profile(&dir, "weekday").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_skips_a_corrupt_file_without_failing_the_whole_call() {
        let dir = temp_dir("corrupt");
        save_schedule_profile(&dir, &sample_profile("good")).unwrap();
        std::fs::write(schedule_profile_path(&dir, "bad"), "{ not valid json").unwrap();

        let listed = list_schedule_profiles(&dir).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "good");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
