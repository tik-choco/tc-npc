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
    let character = npc_core::active_character(&state.ctx.data_dir, &state.ctx.config)
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
    match state.ctx.config.redacted_json() {
        Ok(v) => Json(v).into_response(),
        Err(err) => error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

pub async fn api_put_config(State(state): State<AppState>, body: String) -> Response {
    let mut incoming: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, format!("invalid JSON: {err}")),
    };

    let current = match serde_json::to_value(&*state.ctx.config) {
        Ok(v) => v,
        Err(err) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    };
    restore_masked_secrets(&mut incoming, &current);

    // Validate the merged document actually deserializes as a Config before
    // persisting it, so a malformed PUT can't brick the next startup.
    if let Err(err) = serde_json::from_value::<npc_core::Config>(incoming.clone()) {
        return error_response(StatusCode::BAD_REQUEST, format!("invalid config: {err}"));
    }

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

    // Config is applied on restart only — no hot-swap of the in-memory
    // `Arc<Config>` shared with already-running modules.
    Json(json!({ "ok": true, "note": "restart required" })).into_response()
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

pub async fn api_list_characters(State(state): State<AppState>) -> Response {
    match npc_core::list_characters(&state.ctx.data_dir) {
        Ok(characters) => {
            let active_id = &state.ctx.config.character.active_id;
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
        .unwrap_or_else(|| serde_json::to_value(&*state.ctx.config).unwrap_or_else(|_| json!({})));

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

    Json(json!({ "ok": true, "note": "restart required" })).into_response()
}

fn error_response(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}
