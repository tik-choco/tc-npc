//! Shared building blocks for tc-npc: the in-process bus that replaces
//! Redis pub/sub, the `Module` trait every subsystem implements, app
//! configuration, and character sheet handling.

pub mod bus;
pub mod character;
pub mod chatlog;
pub mod config;
pub mod module;
pub mod mist;
pub mod osc;
pub mod person;
pub mod schedule_profile;
pub mod sound;
pub mod sprite;
pub mod vrm;

pub use bus::{msg, topic, Bus, BusMessage, Envelope};
pub use character::{
    active_character, import_tc_town_export, import_tc_town_export_into, list_characters, load_character, persona_prompt,
    save_character, Avatar, Character, CharacterSheet,
};
pub use chatlog::ChatLogEntry;
pub use config::{
    data_dir, unmask_provider_keys, Config, LlmTask, ModelRef, ProviderConfig, ResolvedLlm,
};
pub use module::{Module, ModuleCtx};
pub use person::{
    delete_person, find_person, list_people, load_person, new_person, normalize_person_name,
    people_dir, person_to_wire, save_person, Person, PersonFact,
};
pub use schedule_profile::{
    delete_schedule_profile, list_schedule_profiles, load_schedule_profile, save_schedule_profile,
    ScheduleProfile,
};
pub use sound::{list_sounds, resolve_sound_path, sound_dir, SoundFile};
pub use sprite::{
    delete_sprite, list_sprites, read_sprite, save_sprite, sprite_dir, sprite_exists, SpriteSheet,
};
pub use vrm::{delete_vrm, list_vrm_models, read_vrm, save_vrm, vrm_dir, vrm_exists, VrmModel};
