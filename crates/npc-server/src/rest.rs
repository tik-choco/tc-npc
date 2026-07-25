//! REST handlers: `/healthz`, `/api/state`, `/api/config`, `/api/characters*`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

use crate::ws::module_flags;
use crate::AppState;

pub async fn healthz() -> impl IntoResponse {
    Json(json!({ "ok": true }))
}

#[derive(Serialize)]
struct CharacterSummary {
    id: String,
    name: String,
    active: bool,
}

pub async fn api_state(State(state): State<AppState>) -> impl IntoResponse {
    let character = npc_core::active_character(&state.ctx.data_dir, &state.current_config())
        .ok()
        .flatten()
        .map(|c| json!({ "id": c.id, "name": c.sheet.name }));

    Json(json!({
        "modules": module_flags(&state.ctx.config),
        "character": character,
        "addr": state.addr,
    }))
}

pub async fn api_get_config(State(state): State<AppState>) -> Response {
    // Serve the latest *saved* config (not the startup snapshot), so the
    // settings UI's load-after-save round-trip sees its own edits.
    match state.current_config().redacted_json() {
        Ok(v) => Json(v).into_response(),
        Err(err) => error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

pub async fn api_put_config(State(state): State<AppState>, body: String) -> Response {
    let mut incoming: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, format!("invalid JSON: {err}")),
    };

    // Restore "***" placeholders from the latest saved config (not the
    // startup snapshot) so a key saved earlier in this session isn't rolled
    // back to the boot-time value by a later round-trip.
    let current = match serde_json::to_value(&*state.current_config()) {
        Ok(v) => v,
        Err(err) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    };
    restore_masked_secrets(&mut incoming, &current);

    // Validate the merged document actually deserializes as a Config before
    // persisting it, so a malformed PUT can't brick the next startup.
    let new_config: npc_core::Config = match serde_json::from_value(incoming.clone()) {
        Ok(c) => c,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, format!("invalid config: {err}")),
    };

    let pretty = match serde_json::to_string_pretty(&incoming) {
        Ok(s) => s,
        Err(err) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    };
    if let Err(err) = std::fs::write(&state.ctx.config_path, pretty) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to write {}: {err}", state.ctx.config_path.display()),
        );
    }

    // Publish the full, unredacted new config on the internal `npc:config`
    // bus topic so modules that can hot-reload (npc-scheduler's
    // announcements, npc-action's locations/routes) pick it up immediately.
    // This does NOT update `state.ctx.config` itself — the in-memory
    // `Arc<Config>` shared with already-running modules is still only
    // refreshed by a restart, hence the note below.
    state
        .ctx
        .bus
        .publish(npc_core::topic::CONFIG, npc_core::msg::CONFIG_UPDATED, &new_config);
    state.set_current_config(new_config);

    Json(json!({
        "ok": true,
        "note": "Schedule and location/route changes take effect immediately; \
                 other changes (API connections, module on/off, etc.) require a restart to take effect."
    }))
    .into_response()
}

/// Walk `incoming` and `current` in lockstep; wherever `incoming` has a
/// `"api_key": "***"` (the redacted placeholder `GET /api/config` returns),
/// replace it with the real value from `current` at the same position, so a
/// PUT that round-trips an unedited redacted blob doesn't wipe out secrets.
fn restore_masked_secrets(incoming: &mut Value, current: &Value) {
    match (incoming, current) {
        (Value::Object(incoming_map), Value::Object(current_map)) => {
            for (key, value) in incoming_map.iter_mut() {
                if key == "api_key" {
                    if let Value::String(s) = value {
                        if s == "***" {
                            if let Some(real) = current_map.get(key) {
                                *value = real.clone();
                            }
                        }
                    }
                    continue;
                }
                if let Some(current_value) = current_map.get(key) {
                    restore_masked_secrets(value, current_value);
                }
            }
        }
        (Value::Array(incoming_arr), Value::Array(current_arr)) => {
            for (i, value) in incoming_arr.iter_mut().enumerate() {
                if let Some(current_value) = current_arr.get(i) {
                    restore_masked_secrets(value, current_value);
                }
            }
        }
        _ => {}
    }
}

/// Body for `POST /api/scheduler/test` — the web UI's "test run" button,
/// mirroring the Go TUI's `[t] Test Playback`.
#[derive(serde::Deserialize)]
struct SchedulerTestRequest {
    /// Index into the saved `config.scheduler.announcements`.
    index: Option<usize>,
    /// Overrides for the saved entry. The UI sends the row exactly as it is
    /// on screen so a test fires what the user is looking at even if the
    /// debounced autosave hasn't landed yet.
    text: Option<String>,
    chime_file: Option<String>,
    /// Same idea for the row's `actions` list. Omitted means "use whatever is
    /// saved"; an empty array means "this row has no actions" — hence
    /// `Option<Vec<_>>` rather than a defaulted `Vec`.
    actions: Option<Vec<npc_core::config::ScheduledAction>>,
}

/// Resolve a test request against the saved config: start from the indexed
/// announcement (if any) and apply the request's overrides on top.
fn resolve_test_announcement(
    req: &SchedulerTestRequest,
    config: &npc_core::Config,
) -> Result<npc_core::config::AnnouncementConfig, String> {
    let mut ann = match req.index {
        Some(index) => config
            .scheduler
            .announcements
            .get(index)
            .cloned()
            .ok_or_else(|| format!("announcement index {index} out of range"))?,
        None if req.text.is_some() || req.actions.is_some() => npc_core::config::AnnouncementConfig {
            time: String::new(),
            text: String::new(),
            chime_file: String::new(),
            volume: 1.0,
            actions: Vec::new(),
        },
        None => return Err("either `index`, `text` or `actions` is required".to_string()),
    };

    if let Some(text) = &req.text {
        ann.text = text.clone();
    }
    if let Some(chime_file) = &req.chime_file {
        ann.chime_file = chime_file.clone();
    }
    if let Some(actions) = &req.actions {
        ann.actions = actions.clone();
    }
    Ok(ann)
}

/// Fire an announcement immediately, regardless of `scheduler.enabled` (the
/// point of a test run is to hear it while the schedule itself is still off).
/// `fired: false` means the entry is a no-op — an announcement with neither
/// text nor actions publishes nothing on the real schedule either. `spoke` /
/// `actions` break that down so the UI can say what actually happened.
pub async fn api_scheduler_test(State(state): State<AppState>, body: String) -> Response {
    let req: SchedulerTestRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, format!("invalid JSON: {err}")),
    };

    let ann = match resolve_test_announcement(&req, &state.current_config()) {
        Ok(a) => a,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, err),
    };

    let outcome = npc_scheduler::fire_announcement(&state.ctx.bus, &ann);
    Json(json!({
        "ok": true,
        "fired": outcome.fired(),
        "spoke": outcome.spoke,
        "actions": outcome.actions,
    }))
    .into_response()
}

/// Body shared by `POST /api/llm/models` and `POST /api/llm/voices`.
#[derive(serde::Deserialize)]
struct LlmProbeRequest {
    #[serde(rename = "baseUrl")]
    base_url: String,
    #[serde(rename = "apiKey")]
    api_key: String,
    section: String,
}

/// If `api_key` is the redacted placeholder `"***"` (the AI settings screen
/// round-tripping a value it fetched from `GET /api/config`), resolve it to
/// the real, saved key for `section`. Otherwise pass it through unchanged —
/// this lets the connection test also work for a key the user just typed in
/// but hasn't saved yet.
fn resolve_probe_api_key(section: &str, api_key: &str, config: &npc_core::Config) -> String {
    if api_key != "***" {
        return api_key.to_string();
    }
    match section {
        "api" => config.api.api_key.clone(),
        "tts" => config.tts.api_key.clone(),
        "stt" => config.stt.api_key.clone(),
        _ => api_key.to_string(),
    }
}

fn parse_llm_probe_request(body: &str) -> Result<LlmProbeRequest, String> {
    serde_json::from_str(body).map_err(|err| format!("invalid JSON: {err}"))
}

pub async fn api_llm_models(State(state): State<AppState>, body: String) -> Response {
    let req = match parse_llm_probe_request(&body) {
        Ok(r) => r,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, err),
    };
    let api_key = resolve_probe_api_key(&req.section, &req.api_key, &state.current_config());
    let client = npc_llm::LlmClient::new(req.base_url, api_key);
    match client.list_models().await {
        Ok(models) => Json(json!({ "models": models })).into_response(),
        Err(err) => error_response(StatusCode::BAD_GATEWAY, err.to_string()),
    }
}

pub async fn api_llm_voices(State(state): State<AppState>, body: String) -> Response {
    let req = match parse_llm_probe_request(&body) {
        Ok(r) => r,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, err),
    };
    let api_key = resolve_probe_api_key(&req.section, &req.api_key, &state.current_config());
    let client = npc_llm::LlmClient::new(req.base_url, api_key);
    match client.list_voices().await {
        Ok(voices) => Json(json!({ "voices": voices })).into_response(),
        Err(err) => error_response(StatusCode::BAD_GATEWAY, err.to_string()),
    }
}

pub async fn api_list_characters(State(state): State<AppState>) -> Response {
    match npc_core::list_characters(&state.ctx.data_dir) {
        Ok(characters) => {
            let current = state.current_config();
            let active_id = &current.character.active_id;
            let out: Vec<CharacterSummary> = characters
                .into_iter()
                .map(|c| CharacterSummary {
                    active: &c.id == active_id,
                    id: c.id,
                    name: c.sheet.name,
                })
                .collect();
            Json(out).into_response()
        }
        Err(err) => error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

pub async fn api_import_characters(State(state): State<AppState>, body: String) -> Response {
    let characters = match npc_core::import_tc_town_export(&body) {
        Ok(c) => c,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, err.to_string()),
    };

    let mut out = Vec::with_capacity(characters.len());
    for character in &characters {
        if let Err(err) = npc_core::save_character(&state.ctx.data_dir, character) {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to save character {}: {err}", character.id),
            );
        }
        out.push(json!({ "id": character.id, "name": character.sheet.name }));
    }

    Json(out).into_response()
}

pub async fn api_activate_character(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let path = &state.ctx.config_path;

    // Read-modify-write the config file on disk, preserving unknown fields
    // where possible; fall back to a full rewrite of the current in-memory
    // config (with the new active_id) if the file is missing or unreadable.
    let mut doc: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_else(|| serde_json::to_value(&*state.current_config()).unwrap_or_else(|_| json!({})));

    if !doc.is_object() {
        doc = json!({});
    }
    let obj = doc.as_object_mut().unwrap();
    let character = obj.entry("character").or_insert_with(|| json!({}));
    if !character.is_object() {
        *character = json!({});
    }
    character
        .as_object_mut()
        .unwrap()
        .insert("active_id".to_string(), json!(id));

    let pretty = match serde_json::to_string_pretty(&doc) {
        Ok(s) => s,
        Err(err) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    };
    if let Err(err) = std::fs::write(path, pretty) {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to write {}: {err}", path.display()),
        );
    }

    // Keep the served config in sync so `GET /api/config` and the character
    // list's `active` flags reflect the activation without a restart (the
    // talk engine's persona still requires one).
    let mut updated = (*state.current_config()).clone();
    updated.character.active_id = id;
    state.set_current_config(updated);

    Json(json!({ "ok": true, "note": "restart required" })).into_response()
}

fn error_response(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_masked_secrets_replaces_placeholder_only() {
        let mut incoming = json!({"api": {"api_key": "***", "model": "gpt-4o-mini"}});
        let current = json!({"api": {"api_key": "sk-real", "model": "old-model"}});
        restore_masked_secrets(&mut incoming, &current);
        assert_eq!(incoming["api"]["api_key"], "sk-real");
        // Non-secret fields are left as sent by the client, not overwritten.
        assert_eq!(incoming["api"]["model"], "gpt-4o-mini");
    }

    #[test]
    fn restore_masked_secrets_leaves_new_key_untouched() {
        let mut incoming = json!({"api": {"api_key": "sk-new"}});
        let current = json!({"api": {"api_key": "sk-real"}});
        restore_masked_secrets(&mut incoming, &current);
        assert_eq!(incoming["api"]["api_key"], "sk-new");
    }

    #[test]
    fn resolve_probe_api_key_resolves_placeholder_per_section() {
        let mut config = npc_core::Config::default();
        config.api.api_key = "sk-api".to_string();
        config.tts.api_key = "sk-tts".to_string();
        config.stt.api_key = "sk-stt".to_string();

        assert_eq!(resolve_probe_api_key("api", "***", &config), "sk-api");
        assert_eq!(resolve_probe_api_key("tts", "***", &config), "sk-tts");
        assert_eq!(resolve_probe_api_key("stt", "***", &config), "sk-stt");
    }

    #[test]
    fn resolve_probe_api_key_passes_through_typed_key() {
        let config = npc_core::Config::default();
        assert_eq!(resolve_probe_api_key("api", "sk-typed-just-now", &config), "sk-typed-just-now");
    }

    fn config_with_announcement() -> npc_core::Config {
        let mut config = npc_core::Config::default();
        config.scheduler.announcements = vec![npc_core::config::AnnouncementConfig {
            time: "09:00".to_string(),
            text: "saved text".to_string(),
            chime_file: "saved.wav".to_string(),
            volume: 1.0,
            actions: vec![npc_core::config::ScheduledAction::Resume],
        }];
        config
    }

    fn test_request(index: Option<usize>, text: Option<&str>) -> SchedulerTestRequest {
        SchedulerTestRequest {
            index,
            text: text.map(str::to_string),
            chime_file: None,
            actions: None,
        }
    }

    #[test]
    fn resolve_test_announcement_uses_saved_entry_by_index() {
        let ann = resolve_test_announcement(&test_request(Some(0), None), &config_with_announcement()).unwrap();
        assert_eq!(ann.text, "saved text");
        assert_eq!(ann.chime_file, "saved.wav");
        assert_eq!(ann.actions, vec![npc_core::config::ScheduledAction::Resume]);
    }

    #[test]
    fn resolve_test_announcement_applies_unsaved_overrides() {
        let req = SchedulerTestRequest {
            index: Some(0),
            text: Some("just typed".to_string()),
            chime_file: Some(String::new()),
            actions: Some(vec![npc_core::config::ScheduledAction::Command {
                text: "go home".to_string(),
            }]),
        };
        let ann = resolve_test_announcement(&req, &config_with_announcement()).unwrap();
        assert_eq!(ann.text, "just typed");
        assert_eq!(ann.chime_file, "");
        assert_eq!(
            ann.actions,
            vec![npc_core::config::ScheduledAction::Command {
                text: "go home".to_string()
            }]
        );
    }

    /// An empty `actions` array is an override to "no actions", not "use the
    /// saved ones" — otherwise deleting the last action row couldn't be
    /// tested before the autosave lands.
    #[test]
    fn resolve_test_announcement_treats_empty_actions_as_an_override() {
        let req = SchedulerTestRequest {
            index: Some(0),
            text: None,
            chime_file: None,
            actions: Some(Vec::new()),
        };
        let ann = resolve_test_announcement(&req, &config_with_announcement()).unwrap();
        assert!(ann.actions.is_empty());
    }

    #[test]
    fn resolve_test_announcement_rejects_out_of_range_index() {
        assert!(resolve_test_announcement(&test_request(Some(3), None), &config_with_announcement()).is_err());
    }

    #[test]
    fn resolve_test_announcement_accepts_text_without_index() {
        let ann =
            resolve_test_announcement(&test_request(None, Some("ad hoc")), &npc_core::Config::default()).unwrap();
        assert_eq!(ann.text, "ad hoc");
    }

    #[test]
    fn resolve_test_announcement_accepts_actions_without_index_or_text() {
        let req = SchedulerTestRequest {
            index: None,
            text: None,
            chime_file: None,
            actions: Some(vec![npc_core::config::ScheduledAction::Suspend]),
        };
        let ann = resolve_test_announcement(&req, &npc_core::Config::default()).unwrap();
        assert!(ann.text.is_empty());
        assert_eq!(ann.actions, vec![npc_core::config::ScheduledAction::Suspend]);
    }

    #[test]
    fn resolve_test_announcement_requires_index_text_or_actions() {
        assert!(resolve_test_announcement(&test_request(None, None), &npc_core::Config::default()).is_err());
    }

    #[test]
    fn parse_llm_probe_request_rejects_invalid_json() {
        assert!(parse_llm_probe_request("not json").is_err());
    }

    #[test]
    fn parse_llm_probe_request_parses_camel_case_fields() {
        let req = parse_llm_probe_request(
            r#"{"baseUrl": "http://localhost:11434", "apiKey": "***", "section": "tts"}"#,
        )
        .unwrap();
        assert_eq!(req.base_url, "http://localhost:11434");
        assert_eq!(req.api_key, "***");
        assert_eq!(req.section, "tts");
    }
}
