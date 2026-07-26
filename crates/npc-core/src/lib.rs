//! Shared building blocks for tc-npc: the in-process bus that replaces
//! Redis pub/sub, the `Module` trait every subsystem implements, app
//! configuration, and character sheet handling.

pub mod bus;
pub mod character;
pub mod chatlog;
pub mod config;
pub mod module;
pub mod osc;
pub mod person;

pub use bus::{msg, topic, Bus, BusMessage, Envelope};
pub use character::{
    active_character, import_tc_town_export, list_characters, load_character, persona_prompt,
    save_character, Character, CharacterSheet,
};
pub use chatlog::ChatLogEntry;
pub use config::{data_dir, Config};
pub use module::{Module, ModuleCtx};
pub use person::{
    delete_person, find_person, list_people, load_person, new_person, normalize_person_name,
    people_dir, person_to_wire, save_person, Person, PersonFact,
};
