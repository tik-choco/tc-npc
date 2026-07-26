//! In-memory tracker for [`npc_core::Person`] records: the npc-memory-side
//! half of the person-memory feature (see the person-memory contract).
//! `npc_core::person` owns the on-disk shape and file I/O; this module owns
//! the *matching* logic (which sighting belongs to which person) and the
//! bookkeeping npc-memory needs to react to chat and vision sightings.
//!
//! [`PersonTracker`] is loaded once at module startup from
//! `npc_core::list_people` and kept as plain owned state in `lib.rs`'s event
//! loop (same pattern as the short-term `buffer`/`summary` state) — no
//! internal locking, since it's only ever touched from that one task.
//!
//! Persisting a change and telling the rest of the app about it
//! ([`PersonTracker::persist_and_publish`]) is kept separate from the
//! matching/merging methods ([`PersonTracker::observe_from_chat`],
//! [`PersonTracker::observe_from_vision`], [`PersonTracker::add_facts`]),
//! which take no [`Bus`] at all — that keeps the interesting logic testable
//! without a bus around, and callers decide when a save+publish is worth
//! doing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use npc_core::bus::{msg, topic};
use npc_core::{Bus, Person, PersonFact};

/// Cap on how many *unnamed* vision-sourced people [`PersonTracker`] will
/// create. A person detector with no name/identity signal can only tell
/// sightings apart by appearance text, which is noisy (lighting, pose,
/// wording) — without a cap, a busy scene could otherwise mint an unbounded
/// number of "someone" records over time. Once this many unnamed
/// vision-sourced people exist, further unmatched anonymous sightings are
/// logged and dropped rather than creating yet another record; a named
/// sighting is never affected by this cap.
const MAX_UNNAMED_VISION_PEOPLE: usize = 32;

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Tracks every known [`Person`], resolving chat/vision sightings to the
/// right record and applying the person-memory config's `max_facts` cap.
pub struct PersonTracker {
    data_dir: PathBuf,
    people: HashMap<String, Person>,
    max_facts: u32,
}

impl PersonTracker {
    /// Load every person under `{data_dir}/people/` (an empty roster if the
    /// directory doesn't exist yet, or listing fails — logged, not fatal).
    /// `max_facts` comes from `config.memory.people.max_facts` and governs
    /// [`PersonTracker::add_facts`].
    pub fn load(data_dir: &Path, max_facts: u32) -> Self {
        let people = npc_core::list_people(data_dir).unwrap_or_else(|err| {
            tracing::warn!(error = %err, "npc-memory: failed to list people, starting with an empty roster");
            Vec::new()
        });
        let people = people.into_iter().map(|p| (p.id.clone(), p)).collect();
        Self {
            data_dir: data_dir.to_path_buf(),
            people,
            max_facts,
        }
    }

    /// Look up a tracked person by id.
    pub fn get(&self, id: &str) -> Option<&Person> {
        self.people.get(id)
    }

    fn snapshot(&self) -> Vec<Person> {
        self.people.values().cloned().collect()
    }

    // -----------------------------------------------------------------
    // chat
    // -----------------------------------------------------------------

    /// Resolve `name` to a person from a chat mention: match against
    /// existing name/aliases (via `npc_core::find_person`), bumping
    /// `last_seen`/`encounter_count` on a hit, or create a new `"chat"`
    /// sourced record if nothing matches. Returns `None` if `name`
    /// normalizes to empty (nothing to resolve, matching
    /// `npc_core::find_person`'s "empty name never matches" rule).
    pub fn observe_from_chat(&mut self, name: &str) -> Option<&Person> {
        if npc_core::normalize_person_name(name).is_empty() {
            return None;
        }

        let existing_id = {
            let snapshot = self.snapshot();
            npc_core::find_person(&snapshot, name).map(|p| p.id.clone())
        };

        let id = match existing_id {
            Some(id) => {
                if let Some(p) = self.people.get_mut(&id) {
                    p.last_seen = now();
                    p.encounter_count += 1;
                }
                id
            }
            None => {
                let p = npc_core::new_person(name, "chat");
                let id = p.id.clone();
                self.people.insert(id.clone(), p);
                id
            }
        };
        self.people.get(&id)
    }

    // -----------------------------------------------------------------
    // vision
    // -----------------------------------------------------------------

    /// Resolve+merge a vision sighting: `name` (may be empty), the latest
    /// `appearance` description, and an optional free-text `note`.
    ///
    /// - A non-empty `name` matches/creates exactly like
    ///   [`PersonTracker::observe_from_chat`] (new records get
    ///   `source: "vision"`).
    /// - An empty `name` instead matches by `appearance`: first against any
    ///   existing person whose current `appearance` is an exact match, then
    ///   against an existing *unnamed vision-sourced* person who has no
    ///   `appearance` recorded yet (an anonymous sighting we haven't been
    ///   able to describe before). If neither matches, a new anonymous
    ///   record is created, subject to [`MAX_UNNAMED_VISION_PEOPLE`].
    ///
    /// On every resolution, `appearance` (if non-empty) overwrites the
    /// person's stored appearance, and `note` (if non-empty) is added as a
    /// `"vision"`-sourced fact via [`PersonTracker::add_facts`]. Returns
    /// `None` if there's nothing usable to observe (all of `name`,
    /// `appearance`, `note` empty) or the anonymous-person cap was hit.
    pub fn observe_from_vision(&mut self, name: &str, appearance: &str, note: &str) -> Option<&Person> {
        if name.trim().is_empty() && appearance.trim().is_empty() && note.trim().is_empty() {
            return None;
        }

        let (id, created) = self.resolve_vision_person(name, appearance)?;

        if let Some(p) = self.people.get_mut(&id) {
            p.last_seen = now();
            // A freshly created record's encounter_count already starts at
            // 1 (this sighting *is* its first encounter); only bump it for
            // a record that already existed before this call.
            if !created {
                p.encounter_count += 1;
            }
            if !appearance.trim().is_empty() {
                p.appearance = appearance.to_string();
            }
        }
        if !note.trim().is_empty() {
            self.add_facts(&id, vec![(note.to_string(), "vision".to_string())]);
        }
        self.people.get(&id)
    }

    /// The matching half of [`PersonTracker::observe_from_vision`]: returns
    /// `(person_id, was_just_created)`, or `None` if an anonymous sighting
    /// couldn't be matched and the [`MAX_UNNAMED_VISION_PEOPLE`] cap has
    /// been reached.
    fn resolve_vision_person(&mut self, name: &str, appearance: &str) -> Option<(String, bool)> {
        if !npc_core::normalize_person_name(name).is_empty() {
            let existing_id = {
                let snapshot = self.snapshot();
                npc_core::find_person(&snapshot, name).map(|p| p.id.clone())
            };
            if let Some(id) = existing_id {
                return Some((id, false));
            }
            let p = npc_core::new_person(name, "vision");
            let id = p.id.clone();
            self.people.insert(id.clone(), p);
            return Some((id, true));
        }

        if !appearance.trim().is_empty() {
            if let Some(p) = self.people.values().find(|p| p.appearance == appearance) {
                return Some((p.id.clone(), false));
            }
        }
        if let Some(p) = self
            .people
            .values()
            .find(|p| p.name.is_empty() && p.source == "vision" && p.appearance.is_empty())
        {
            return Some((p.id.clone(), false));
        }

        let unnamed_vision_count = self
            .people
            .values()
            .filter(|p| p.name.is_empty() && p.source == "vision")
            .count();
        if unnamed_vision_count >= MAX_UNNAMED_VISION_PEOPLE {
            tracing::warn!(
                limit = MAX_UNNAMED_VISION_PEOPLE,
                "npc-memory: unnamed vision person cap reached, dropping unmatched sighting"
            );
            return None;
        }

        let p = npc_core::new_person("", "vision");
        let id = p.id.clone();
        self.people.insert(id.clone(), p);
        Some((id, true))
    }

    // -----------------------------------------------------------------
    // facts
    // -----------------------------------------------------------------

    /// Append `facts` (each a `(text, source)` pair) to `person_id`'s fact
    /// list. A `text` that's blank, or already present verbatim, is
    /// skipped (no duplicate facts). After appending, if the list exceeds
    /// `max_facts` (from the config passed to [`PersonTracker::load`]), the
    /// oldest entries (front of the list, since facts are always appended
    /// at the end) are dropped until it fits. A `person_id` that isn't
    /// tracked is a no-op.
    pub fn add_facts(&mut self, person_id: &str, facts: Vec<(String, String)>) {
        let Some(p) = self.people.get_mut(person_id) else {
            return;
        };

        for (text, source) in facts {
            let text = text.trim();
            if text.is_empty() || p.facts.iter().any(|f| f.text == text) {
                continue;
            }
            p.facts.push(PersonFact {
                text: text.to_string(),
                source,
                created_at: now(),
            });
        }

        let max = self.max_facts as usize;
        if p.facts.len() > max {
            let excess = p.facts.len() - max;
            p.facts.drain(0..excess);
        }
    }

    // -----------------------------------------------------------------
    // persistence + publish
    // -----------------------------------------------------------------

    /// Save `person_id`'s current state to disk (`npc_core::save_person`)
    /// and publish it as `PERSON_UPDATED` on `topic::UI`, wire-encoded via
    /// `npc_core::person_to_wire`. Takes `bus` as a parameter (rather than
    /// storing one on `PersonTracker`) so the matching/merging methods above
    /// stay testable without a real bus. A `person_id` that isn't tracked is
    /// a no-op.
    pub fn persist_and_publish(&self, bus: &Bus, person_id: &str) {
        let Some(p) = self.people.get(person_id) else {
            return;
        };
        if let Err(err) = npc_core::save_person(&self.data_dir, p) {
            tracing::warn!(person_id, error = %err, "npc-memory: failed to save person record");
        }
        bus.publish(topic::UI, msg::PERSON_UPDATED, npc_core::person_to_wire(p));
    }

    // -----------------------------------------------------------------
    // prompt text
    // -----------------------------------------------------------------

    /// Render `person`'s profile as the fixed-Japanese text injected into
    /// the prompt as `{{person_memory}}` (published as `person_memory` on
    /// `topic::MEM`). This is deliberately not run through
    /// `with_language_instruction`/`config.language`: it's a fixed Japanese
    /// template, not an LLM prompt, and npc-talk's own language instruction
    /// already covers translating the model's *reply* — see the
    /// person-memory contract §B. Empty elements (no facts, no appearance,
    /// no notes) are omitted line-by-line rather than left blank.
    pub fn profile_text(&self, person: &Person) -> String {
        let mut lines = Vec::new();

        if person.name.is_empty() {
            lines.push(format!("名前不明の相手です(これまで{}回接触)。", person.encounter_count));
        } else {
            lines.push(format!(
                "相手は「{}」さんです(これまで{}回会話)。",
                person.name, person.encounter_count
            ));
        }

        if !person.facts.is_empty() {
            lines.push("覚えていること:".to_string());
            for fact in &person.facts {
                lines.push(format!("- {}", fact.text));
            }
        }

        if !person.appearance.is_empty() {
            lines.push(format!("見た目: {}", person.appearance));
        }

        if !person.notes.is_empty() {
            lines.push(format!("メモ: {}", person.notes));
        }

        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker(max_facts: u32) -> PersonTracker {
        PersonTracker {
            data_dir: std::env::temp_dir().join(format!("npc-memory-person-test-{}", uuid::Uuid::new_v4())),
            people: HashMap::new(),
            max_facts,
        }
    }

    #[test]
    fn observe_from_chat_creates_then_matches_on_repeat() {
        let mut t = tracker(20);

        let p1 = t.observe_from_chat("太郎さん").unwrap();
        let id = p1.id.clone();
        assert_eq!(p1.name, "太郎さん");
        assert_eq!(p1.source, "chat");
        assert_eq!(p1.encounter_count, 1);

        // Same person, different honorific -> matched, not duplicated.
        let p2 = t.observe_from_chat("太郎くん").unwrap();
        assert_eq!(p2.id, id);
        assert_eq!(p2.encounter_count, 2);
        assert_eq!(t.people.len(), 1);
    }

    #[test]
    fn observe_from_chat_empty_name_is_none() {
        let mut t = tracker(20);
        assert!(t.observe_from_chat("").is_none());
        assert!(t.observe_from_chat("   ").is_none());
        assert!(t.people.is_empty());
    }

    #[test]
    fn observe_from_vision_matches_by_name_like_chat() {
        let mut t = tracker(20);
        let created = t.observe_from_vision("花子", "赤い帽子", "").unwrap();
        let id = created.id.clone();
        assert_eq!(created.source, "vision");
        assert_eq!(created.appearance, "赤い帽子");
        assert_eq!(created.encounter_count, 1);

        let seen_again = t.observe_from_vision("花子さん", "青い帽子", "").unwrap();
        assert_eq!(seen_again.id, id);
        assert_eq!(seen_again.encounter_count, 2);
        // appearance overwritten with the latest observation.
        assert_eq!(seen_again.appearance, "青い帽子");
    }

    #[test]
    fn observe_from_vision_matches_unnamed_by_exact_appearance() {
        let mut t = tracker(20);
        let first = t.observe_from_vision("", "青いパーカーの男性", "").unwrap();
        let id = first.id.clone();

        let second = t.observe_from_vision("", "青いパーカーの男性", "").unwrap();
        assert_eq!(second.id, id, "same appearance text should resolve to the same anonymous person");
        assert_eq!(second.encounter_count, 2);
        assert_eq!(t.people.len(), 1);
    }

    #[test]
    fn observe_from_vision_reuses_unnamed_person_with_no_appearance_yet() {
        let mut t = tracker(20);
        // First sighting: no name, no appearance yet (e.g. seen only briefly).
        let first = t.observe_from_vision("", "", "手を振っている").unwrap();
        let id = first.id.clone();
        assert!(first.appearance.is_empty());

        // A later sighting that does have an appearance should attach to
        // that same still-undescribed anonymous person rather than minting
        // a new one.
        let second = t.observe_from_vision("", "青いパーカーの男性", "").unwrap();
        assert_eq!(second.id, id);
        assert_eq!(second.appearance, "青いパーカーの男性");
        assert_eq!(t.people.len(), 1);
    }

    #[test]
    fn observe_from_vision_all_blank_is_none() {
        let mut t = tracker(20);
        assert!(t.observe_from_vision("", "", "").is_none());
        assert!(t.people.is_empty());
    }

    #[test]
    fn observe_from_vision_caps_unnamed_person_creation() {
        let mut t = tracker(20);
        for i in 0..MAX_UNNAMED_VISION_PEOPLE {
            let appearance = format!("見た目-{i}");
            let person = t.observe_from_vision("", &appearance, "").unwrap();
            assert_eq!(person.appearance, appearance);
        }
        assert_eq!(t.people.len(), MAX_UNNAMED_VISION_PEOPLE);

        // One more distinct, unmatched anonymous sighting: dropped, cap held.
        let over_cap = t.observe_from_vision("", "見た目-over-cap", "");
        assert!(over_cap.is_none());
        assert_eq!(t.people.len(), MAX_UNNAMED_VISION_PEOPLE);

        // A *named* sighting is unaffected by the anonymous-person cap.
        let named = t.observe_from_vision("次郎", "見た目-named", "").unwrap();
        assert_eq!(named.name, "次郎");
        assert_eq!(t.people.len(), MAX_UNNAMED_VISION_PEOPLE + 1);
    }

    #[test]
    fn add_facts_dedups_and_trims_oldest_past_max_facts() {
        let mut t = tracker(2);
        let id = t.observe_from_chat("太郎").unwrap().id.clone();

        t.add_facts(&id, vec![("コーヒーが好き".to_string(), "chat".to_string())]);
        t.add_facts(&id, vec![("犬を飼っている".to_string(), "chat".to_string())]);
        // Duplicate text: not added again.
        t.add_facts(&id, vec![("コーヒーが好き".to_string(), "chat".to_string())]);
        assert_eq!(t.get(&id).unwrap().facts.len(), 2);

        // Exceeding max_facts (2) drops the oldest ("コーヒーが好き").
        t.add_facts(&id, vec![("猫アレルギーがある".to_string(), "chat".to_string())]);
        let facts = &t.get(&id).unwrap().facts;
        assert_eq!(facts.len(), 2);
        assert_eq!(facts[0].text, "犬を飼っている");
        assert_eq!(facts[1].text, "猫アレルギーがある");
    }

    #[test]
    fn add_facts_ignores_blank_text_and_unknown_person() {
        let mut t = tracker(20);
        let id = t.observe_from_chat("太郎").unwrap().id.clone();
        t.add_facts(&id, vec![("  ".to_string(), "chat".to_string())]);
        assert!(t.get(&id).unwrap().facts.is_empty());

        // Unknown person id: no panic, just a no-op.
        t.add_facts("does-not-exist", vec![("x".to_string(), "chat".to_string())]);
    }

    #[test]
    fn observe_from_vision_note_becomes_a_fact() {
        let mut t = tracker(20);
        let id = t.observe_from_vision("", "帽子の男性", "手を振っている").unwrap().id.clone();
        let facts = &t.get(&id).unwrap().facts;
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].text, "手を振っている");
        assert_eq!(facts[0].source, "vision");
    }

    #[test]
    fn profile_text_includes_only_non_empty_sections() {
        let t = tracker(20);
        let mut person = npc_core::new_person("太郎", "chat");
        person.encounter_count = 12;
        person.facts.push(PersonFact {
            text: "コーヒーが好き".to_string(),
            source: "chat".to_string(),
            created_at: 0,
        });
        person.facts.push(PersonFact {
            text: "犬を飼っている".to_string(),
            source: "chat".to_string(),
            created_at: 0,
        });
        person.appearance = "青いパーカー".to_string();

        let text = t.profile_text(&person);
        assert_eq!(
            text,
            "相手は「太郎」さんです(これまで12回会話)。\n覚えていること:\n- コーヒーが好き\n- 犬を飼っている\n見た目: 青いパーカー"
        );
        assert!(!text.contains("メモ"));
    }

    #[test]
    fn profile_text_handles_unnamed_person_with_nothing_recorded() {
        let t = tracker(20);
        let person = npc_core::new_person("", "vision");
        let text = t.profile_text(&person);
        assert_eq!(text, "名前不明の相手です(これまで1回接触)。");
    }

    #[test]
    fn profile_text_includes_notes_when_present() {
        let t = tracker(20);
        let mut person = npc_core::new_person("花子", "manual");
        person.notes = "常連さん".to_string();
        let text = t.profile_text(&person);
        assert!(text.ends_with("メモ: 常連さん"));
    }
}
