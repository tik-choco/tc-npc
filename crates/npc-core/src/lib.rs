//! Shared building blocks for tc-npc: the in-process bus that replaces
//! Redis pub/sub, the `Module` trait every subsystem implements, app
//! configuration, and character sheet handling.

pub mod bus;
pub mod character;
pub mod config;
pub mod module;

pub use bus::{msg, topic, Bus, BusMessage, Envelope};
pub use character::{
    active_character, import_tc_town_export, list_characters, load_character, persona_prompt,
    save_character, Character, CharacterSheet,
};
pub use config::{data_dir, Config};
pub use module::{Module, ModuleCtx};
