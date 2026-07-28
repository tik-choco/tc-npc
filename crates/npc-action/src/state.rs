//! Shared state for the action module: the OSC client, navigator/autopilot,
//! LLM client + chat history, and the single "active background task" slot
//! that `ExecuteActions`/`stopActiveTask` used to guard in Go.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use npc_core::config::{LocationConfig, RouteConfig};
use npc_core::{Bus, Config};
use npc_llm::{ChatMessage, LlmClient};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::autopilot::Autopilot;
use crate::controller::Controller;
use crate::navigator::Navigator;
use crate::osc::VrcClient;

/// `config.action.locations` / `config.action.routes`, split out of the
/// otherwise-immutable `Config` snapshot into their own lock so a
/// `CONFIG_UPDATED` bus message (see `crate::handle_config_updated`) can
/// hot-reload them: dispatcher lookups (`go`, `route`, the `locations`/
/// `routes` commands) and `Autopilot::run_route`'s `find_location` always
/// read whatever is current at the moment they're called. A route already
/// in flight keeps running against the snapshot it was handed when it
/// started (see `dispatcher::run_command`'s `route` arm) — only the *next*
/// lookup picks up new data, which is all the hot-reload contract requires.
///
/// `action.osc_address` and `action.enabled` are deliberately NOT part of
/// this: both are read once at module startup (`ActionModule::run`
/// connecting the OSC client, and `src/main.rs` deciding whether to spawn
/// this module at all) and stay fixed for the process lifetime.
#[derive(Debug, Clone, Default)]
pub struct MapData {
    pub locations: Vec<LocationConfig>,
    pub routes: Vec<RouteConfig>,
}

impl MapData {
    pub fn from_config(config: &Config) -> Self {
        Self {
            locations: config.action.locations.clone(),
            routes: config.action.routes.clone(),
        }
    }

    /// Case-insensitive location lookup by name.
    pub fn find_location(&self, name: &str) -> Option<LocationConfig> {
        self.locations
            .iter()
            .find(|l| l.name.to_lowercase() == name.to_lowercase())
            .cloned()
    }

    /// Case-insensitive route lookup by name.
    pub fn find_route(&self, name: &str) -> Option<RouteConfig> {
        self.routes.iter().find(|r| r.name.to_lowercase() == name.to_lowercase()).cloned()
    }
}

/// A running cancellable action sequence (ports the Go CLI's
/// `activeStop`/`activeTaskDone` channel pair).
pub struct ActiveTask {
    pub token: CancellationToken,
    pub handle: tokio::task::JoinHandle<()>,
}

pub struct ActionState {
    pub bus: Bus,
    pub config: Arc<Config>,
    /// Parent of every action-sequence's cancellation token, so process
    /// shutdown also cancels any in-flight movement.
    pub shutdown: CancellationToken,

    pub vrc: Arc<VrcClient>,
    pub controller: Controller,
    pub navigator: Arc<Navigator>,
    pub autopilot: Autopilot,

    /// Hot-reloadable locations/routes — see [`MapData`]'s doc comment.
    pub map_data: Arc<RwLock<MapData>>,

    pub llm: LlmClient,
    /// Resolved once at startup via `Config::resolve_llm(LlmTask::Action)` —
    /// see `ActionModule::run`. Kept alongside `llm` since `chat_with_action`
    /// needs the model name on every request.
    pub model: String,
    pub static_system_prompt: String,
    pub chat_history: AsyncMutex<Vec<ChatMessage>>,
    /// Guards `ChatWithAction`: only one NL round-trip runs at a time: new
    /// requests are dropped (and logged) while one is in flight.
    pub chat_busy: AtomicBool,

    pub active: Mutex<Option<ActiveTask>>,
}

impl ActionState {
    /// Publishes an `npc:ui` / `action_log` line, mirroring every
    /// `fmt.Println`/`fmt.Printf` the Go CLI used to write to stdout.
    pub fn log(&self, text: impl Into<String>) {
        self.bus.publish("npc:ui", "action_log", serde_json::json!({ "text": text.into() }));
    }

    /// Current locations, cloned out of the hot-reloadable lock.
    pub fn locations(&self) -> Vec<LocationConfig> {
        self.map_data.read().unwrap().locations.clone()
    }

    /// Current routes, cloned out of the hot-reloadable lock.
    pub fn routes(&self) -> Vec<RouteConfig> {
        self.map_data.read().unwrap().routes.clone()
    }

    /// Case-insensitive location lookup by name (ports the lookup
    /// `dispatcher`'s `go` command used to do directly against
    /// `config.action.locations`).
    pub fn find_location(&self, name: &str) -> Option<LocationConfig> {
        self.map_data.read().unwrap().find_location(name)
    }

    /// Case-insensitive route lookup by name (ports the lookup
    /// `dispatcher`'s `route` command used to do directly against
    /// `config.action.routes`).
    pub fn find_route(&self, name: &str) -> Option<RouteConfig> {
        self.map_data.read().unwrap().find_route(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(name: &str) -> LocationConfig {
        LocationConfig { name: name.to_string(), x: 1.0, y: 2.0, heading: 90.0 }
    }

    #[test]
    fn map_data_from_config_clones_locations_and_routes() {
        let mut config = Config::default();
        config.action.locations = vec![location("Home")];
        config.action.routes = vec![RouteConfig { name: "Patrol".to_string(), r#loop: true, waypoints: vec![] }];

        let map_data = MapData::from_config(&config);
        assert_eq!(map_data.locations.len(), 1);
        assert_eq!(map_data.locations[0].name, "Home");
        assert_eq!(map_data.routes.len(), 1);
        assert_eq!(map_data.routes[0].name, "Patrol");
    }

    /// Simulates `crate::handle_config_updated`'s lock-swap: a reload with a
    /// new `Config` fully replaces the previous locations/routes, and a
    /// lookup taken after the swap sees the new data.
    #[test]
    fn map_data_lock_swap_replaces_previous_contents() {
        let mut first = Config::default();
        first.action.locations = vec![location("Home")];
        let map_data = std::sync::RwLock::new(MapData::from_config(&first));
        assert_eq!(map_data.read().unwrap().locations[0].name, "Home");

        let mut second = Config::default();
        second.action.locations = vec![location("Shrine"), location("Cafe")];
        *map_data.write().unwrap() = MapData::from_config(&second);

        let reloaded = map_data.read().unwrap();
        assert_eq!(reloaded.locations.len(), 2);
        assert_eq!(reloaded.locations[0].name, "Shrine");
        assert_eq!(reloaded.locations[1].name, "Cafe");
    }

    #[test]
    fn find_location_and_find_route_are_case_insensitive() {
        let mut config = Config::default();
        config.action.locations = vec![location("Home")];
        config.action.routes = vec![RouteConfig { name: "Patrol".to_string(), r#loop: false, waypoints: vec![] }];
        let map_data = MapData::from_config(&config);

        assert!(map_data.find_location("home").is_some());
        assert!(map_data.find_location("HOME").is_some());
        assert!(map_data.find_location("nowhere").is_none());

        assert!(map_data.find_route("PATROL").is_some());
        assert!(map_data.find_route("patrol").is_some());
        assert!(map_data.find_route("nope").is_none());
    }
}
