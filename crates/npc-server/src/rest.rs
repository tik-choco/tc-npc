//! REST handlers: `/healthz`, `/api/state`, `/api/config`, `/api/characters*`.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
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
    /// The character's assigned VRM (`{"kind","file"}`), or null. The キャラ
    /// tab shows which model each character uses, so this rides along with
    /// the list rather than needing a per-character fetch.
    avatar: Option<npc_core::Avatar>,
}

/// The `{"id","name","avatar"}` shape `/api/state` and the character list
/// use for a character reference. `avatar` is the character's *own*
/// assignment, which is not necessarily the model on screen — see
/// [`crate::ws::avatar_ref`] for that.
fn character_json(c: &npc_core::Character) -> Value {
    json!({ "id": c.id, "name": c.sheet.name, "avatar": c.avatar })
}

pub async fn api_state(State(state): State<AppState>) -> impl IntoResponse {
    let character = npc_core::active_character(&state.ctx.data_dir, &state.current_config())
        .ok()
        .flatten()
        .map(|c| character_json(&c));

    Json(json!({
        "modules": module_flags(&state.current_config()),
        "character": character,
        // Resolved the same way as the `hello` frame's, so a page load and a
        // socket connect never disagree about which model to show.
        "avatar": crate::ws::avatar_ref(&state),
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

    let current_config = state.current_config();

    // `providers[]` needs its own pass: the web UI's provider list can be
    // reordered independently of `current_config`, so it must be restored by
    // matching `id`, not array position (npc_core::unmask_provider_keys).
    // This runs BEFORE the generic positional `restore_masked_secrets` below
    // on purpose: unmask_provider_keys turns a matched provider's "***" into
    // its real key first, so by the time restore_masked_secrets walks the
    // same array it finds a non-"***" value there and leaves it untouched —
    // it never gets a chance to instead pair that provider with whatever
    // happens to sit at the same index in `current_config`. Doing it in the
    // other order would let a reordered provider's key get positionally
    // clobbered before the id-based fix could see the still-masked "***".
    npc_core::unmask_provider_keys(&mut incoming, &current_config);

    // Restore "***" placeholders from the latest saved config (not the
    // startup snapshot) so a key saved earlier in this session isn't rolled
    // back to the boot-time value by a later round-trip. This still covers
    // the legacy api/tts/stt/vision sections, plus any providers[] entry
    // unmask_provider_keys above couldn't resolve (e.g. an id not present in
    // current_config at all), as a positional best-effort fallback.
    let current = match serde_json::to_value(&*current_config) {
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
    // announcements, npc-action's locations/routes, npc-speech's whole
    // stt/tts setup) pick it up immediately. This does NOT update
    // `state.ctx.config` itself — the in-memory `Arc<Config>` shared with
    // already-running modules is still only refreshed by a restart, hence the
    // note below; hot-reloading modules keep their own copy of the latest
    // config instead.
    state
        .ctx
        .bus
        .publish(npc_core::topic::CONFIG, npc_core::msg::CONFIG_UPDATED, &new_config);
    state.set_current_config(new_config);

    Json(json!({
        "ok": true,
        "note": "Schedule, location/route and speech (stt/tts on-off, devices, endpoints, VAD) \
                 changes take effect immediately; other changes (API connections, module on/off, \
                 etc.) require a restart to take effect."
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
///
/// `provider_id` is the new provider/preset model's way of naming a saved
/// connection; `section` is the pre-provider/preset spelling (`"api"` /
/// `"tts"` / `"stt"`), kept optional rather than required so the AI settings
/// screen's existing per-section callers (SettingsView's `ModelPicker`, not
/// yet migrated) keep working unchanged. Both are only consulted when
/// `api_key == "***"`; see [`resolve_probe_api_key`].
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LlmProbeRequest {
    base_url: String,
    api_key: String,
    #[serde(default)]
    section: Option<String>,
    #[serde(default)]
    provider_id: Option<String>,
}

/// If `api_key` is the redacted placeholder `"***"` (the AI settings screen
/// round-tripping a value it fetched from `GET /api/config`), resolve it to
/// a real, saved key. Otherwise pass it through unchanged — this lets the
/// connection test also work for a key the user just typed in but hasn't
/// saved yet.
///
/// Resolution order for the placeholder: `provider_id` (looked up in
/// `config.providers[]`) takes priority when present, since it names the
/// specific connection the caller means; falling back to `section` keeps the
/// pre-provider/preset callers working exactly as before. `section` itself
/// defaults to `"api"` when omitted, matching the only section every legacy
/// caller could previously reach without one.
fn resolve_probe_api_key(
    provider_id: Option<&str>,
    section: Option<&str>,
    api_key: &str,
    config: &npc_core::Config,
) -> String {
    if api_key != "***" {
        return api_key.to_string();
    }
    if let Some(id) = provider_id {
        if let Some(provider) = config.provider(id) {
            return provider.api_key.clone();
        }
    }
    match section.unwrap_or("api") {
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
    let api_key = resolve_probe_api_key(
        req.provider_id.as_deref(),
        req.section.as_deref(),
        &req.api_key,
        &state.current_config(),
    );
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
    let api_key = resolve_probe_api_key(
        req.provider_id.as_deref(),
        req.section.as_deref(),
        &req.api_key,
        &state.current_config(),
    );
    let client = npc_llm::LlmClient::new(req.base_url, api_key);
    match client.list_voices().await {
        Ok(voices) => Json(json!({ "voices": voices })).into_response(),
        Err(err) => error_response(StatusCode::BAD_GATEWAY, err.to_string()),
    }
}

/// Input/output audio endpoints visible to the server's host, for the チャット
/// sidebar's 音声 device pickers. What gets *saved* is a device name into
/// `config.speech.{input,output}_device`, which npc-speech matches as a
/// case-insensitive substring at startup (see npc-speech's device.rs) — so
/// this listing is a convenience, not a handle the client has to hold on to.
///
/// cpal enumeration blocks on the OS audio API, hence the blocking pool.
pub async fn api_audio_devices() -> Response {
    match tokio::task::spawn_blocking(npc_speech::list_devices).await {
        Ok(devices) => Json(devices).into_response(),
        Err(err) => error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
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
                    avatar: c.avatar,
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

/// Read-modify-write one field of the config file's `character` section,
/// preserving every other field (including ones this build doesn't know
/// about); falls back to a full rewrite of the current in-memory config if
/// the file is missing or unreadable. Returns an error message on failure.
fn write_character_config_field(state: &AppState, key: &str, value: Value) -> Result<(), String> {
    let path = &state.ctx.config_path;

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
    character.as_object_mut().unwrap().insert(key.to_string(), value);

    let pretty = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
    std::fs::write(path, pretty).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

pub async fn api_activate_character(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    if let Err(err) = write_character_config_field(&state, "active_id", json!(id)) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, err);
    }

    // Keep the served config in sync so `GET /api/config` and the character
    // list's `active` flags reflect the activation without a restart.
    let mut updated = (*state.current_config()).clone();
    updated.character.active_id = id;

    // Publish the full config on `npc:config`, exactly as `api_put_config`
    // does, so npc-talk's persona hot-reload sees the new `active_id` and
    // switches immediately — this is what removes the restart requirement
    // the response used to warn about. `broadcast_avatar` below is a
    // separate, WS-only notification for the model on screen; it was never
    // the thing standing between this endpoint and "no restart needed".
    state
        .ctx
        .bus
        .publish(npc_core::topic::CONFIG, npc_core::msg::CONFIG_UPDATED, &updated);
    state.set_current_config(updated);
    // Which character is active decides which avatar resolves, so this can
    // change the model on screen even though nothing about the models moved.
    broadcast_avatar(&state);

    Json(json!({ "ok": true, "note": "persona and avatar switched immediately; no restart required" })).into_response()
}

/// Body for `PUT /api/vrm/default`. `null`/`""` clears the default avatar.
#[derive(serde::Deserialize)]
struct SetDefaultAvatarRequest {
    #[serde(default)]
    file: Option<String>,
}

/// `PUT /api/vrm/default`: set the avatar shown when no active character
/// supplies one (`config.character.avatar_file`).
///
/// This is what makes a VRM usable on its own. tc-npc holds a conversation
/// with no character sheet loaded, so gating the avatar behind "import a
/// tc-town export, then assign" would put a step in front of the one thing
/// the feature is for — importing a model and talking to it.
pub async fn api_set_default_avatar(State(state): State<AppState>, body: String) -> Response {
    let req: SetDefaultAvatarRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, format!("invalid JSON: {err}")),
    };

    let file = req.file.unwrap_or_default();
    let file = file.trim().to_string();
    if !file.is_empty() {
        match npc_core::vrm_exists(&state.ctx.data_dir, &file) {
            Ok(true) => {}
            Ok(false) => return error_response(StatusCode::NOT_FOUND, format!("vrm not found: {file}")),
            Err(err) => return error_response(vrm_error_status(&err), err.to_string()),
        }
    }

    if let Err(err) = write_character_config_field(&state, "avatar_file", json!(file)) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, err);
    }

    // Served config kept in sync so the next `hello` (and `GET /api/vrm`'s
    // `default`) reflect the change without a restart — the avatar is a
    // browser-side concern, so nothing else has to reload for it.
    let mut updated = (*state.current_config()).clone();
    updated.character.avatar_file = file.clone();
    state.set_current_config(updated);
    broadcast_avatar(&state);

    Json(json!({ "ok": true, "file": file })).into_response()
}

fn error_response(status: StatusCode, message: String) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

// ---------------------------------------------------------------------
// /api/vrm — the local VRM model folder
// ---------------------------------------------------------------------

/// Tell every open tab which model to display now.
///
/// Called after anything that can change the answer: assigning or clearing a
/// character's avatar, changing the standalone default, or activating a
/// different character. Without it the avatar would only ever arrive in
/// `hello`, so a tab that was already open when the model was chosen would
/// sit with its avatar layout disabled until reloaded — which is exactly how
/// it first behaved.
fn broadcast_avatar(state: &AppState) {
    state.hub.broadcast(&crate::protocol::ServerMsg::Avatar {
        avatar: crate::ws::avatar_ref(state),
    });
}

/// A name that failed [`npc_core::vrm::validate_vrm_file_name`] is a client
/// mistake (a crafted URL, or a file the user tried to add that isn't a
/// `.vrm`), not a server fault — same split as `person_error_status`.
fn vrm_error_status(err: &anyhow::Error) -> StatusCode {
    let text = err.to_string();
    if text.contains("invalid vrm file name") || text.contains("not a .vrm file") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// `GET /api/vrm`: every `.vrm` in `{data_dir}/vrm/`. The folder *is* the
/// library — a model dropped in there by hand appears here with no import
/// step — so this is a plain directory listing, not a database read.
pub async fn api_list_vrm(State(state): State<AppState>) -> Response {
    match npc_core::list_vrm_models(&state.ctx.data_dir) {
        Ok(models) => Json(json!({
            "models": models,
            "dir": npc_core::vrm_dir(&state.ctx.data_dir),
            // The standalone default (`config.character.avatar_file`), so the
            // キャラ tab can mark which model is in use with no character.
            "default": state.current_config().character.avatar_file,
        }))
        .into_response(),
        Err(err) => error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

/// `Cache-Control` sent with model bytes and with their `304`s alike.
///
/// `no-cache` does *not* mean "don't cache" (that's `no-store`) — per RFC
/// 7234 it means "cache it, but always revalidate with the server before
/// using the cached copy." Paired with the strong `ETag` below, that
/// revalidation is a bodyless `304` when nothing changed, so this keeps the
/// cheap-repeat-load win of the old `max-age=3600` while fixing what that
/// value actually got wrong: with only a `max-age`, overwriting a model
/// under its existing file name (the キャラ tab's replace-by-upload path)
/// left every tab showing the stale bytes for up to an hour with no way to
/// notice, because the browser had no reason to even ask. A short `max-age`
/// would only narrow that window, not close it — `no-cache` closes it,
/// because every load now asks.
const VRM_FILE_CACHE_CONTROL: &str = "private, no-cache";

/// Build a strong, quoted `ETag` from a model's size and modified time —
/// enough to change whenever the bytes could have (including a same-size
/// overwrite), without reading the file itself. Hex is just a compact,
/// opaque encoding; nothing about the value is meant to be parsed back.
fn vrm_etag(stat: &npc_core::vrm::VrmFileStat) -> String {
    let modified = stat.modified.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("\"{:x}-{:x}-{:x}\"", stat.size, modified.as_secs(), modified.subsec_nanos())
}

/// Whether the raw `If-None-Match` header value covers `etag`: either the
/// wildcard `*`, or `etag` appearing among the header's comma-separated list
/// (per RFC 7232 §3.2), ignoring a leading `W/` weak-validator marker on
/// each candidate — we only ever hand out strong tags, but a client is free
/// to echo one back marked weak.
fn if_none_match_covers(if_none_match: &str, etag: &str) -> bool {
    if if_none_match.trim() == "*" {
        return true;
    }
    if_none_match
        .split(',')
        .any(|candidate| candidate.trim().trim_start_matches("W/") == etag)
}

/// `GET /api/vrm/file/:file`: the raw model bytes, for the browser's VRM
/// loader. Served from disk, but conditionally: the response carries a
/// strong `ETag` built from the file's size and modified time
/// ([`vrm_etag`]), and a request whose `If-None-Match` already covers that
/// tag gets back a bodyless `304` instead of the file. This is what makes
/// [`VRM_FILE_CACHE_CONTROL`]'s "revalidate every time" affordable — most
/// loads cost one small round trip instead of 10-50MB — and, more
/// importantly, what makes a replaced model show up on the very next load.
///
/// The `304` branch deliberately never calls [`npc_core::read_vrm`]:
/// building the validator only needs a stat, and reading megabytes of bytes
/// just to discard them would defeat the entire point of this handler.
pub async fn api_get_vrm_file(State(state): State<AppState>, Path(file): Path<String>, headers: HeaderMap) -> Response {
    let stat = match npc_core::vrm::vrm_file_stat(&state.ctx.data_dir, &file) {
        Ok(Some(stat)) => stat,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, format!("vrm not found: {file}")),
        Err(err) => return error_response(vrm_error_status(&err), err.to_string()),
    };
    let etag = vrm_etag(&stat);

    let not_modified = headers
        .get(axum::http::header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| if_none_match_covers(v, &etag));
    if not_modified {
        return (
            StatusCode::NOT_MODIFIED,
            [
                (axum::http::header::ETAG, etag),
                (axum::http::header::CACHE_CONTROL, VRM_FILE_CACHE_CONTROL.to_string()),
            ],
        )
            .into_response();
    }

    match npc_core::read_vrm(&state.ctx.data_dir, &file) {
        Ok(Some(bytes)) => (
            [
                (axum::http::header::CONTENT_TYPE, "model/gltf-binary".to_string()),
                (axum::http::header::CACHE_CONTROL, VRM_FILE_CACHE_CONTROL.to_string()),
                (axum::http::header::ETAG, etag),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, format!("vrm not found: {file}")),
        Err(err) => error_response(vrm_error_status(&err), err.to_string()),
    }
}

/// `POST /api/vrm/:file`: copy an uploaded `.vrm` into the model folder.
/// Purely a convenience over dropping the file in there by hand — the body
/// is the raw model bytes, and an existing model of the same name is
/// replaced (which is also what overwriting the file would do).
pub async fn api_upload_vrm(
    State(state): State<AppState>,
    Path(file): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if body.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "empty request body".to_string());
    }
    match npc_core::save_vrm(&state.ctx.data_dir, &file, &body) {
        Ok(model) => {
            // Adding a model can make an avatar appear: a character (or the
            // default) may already name this file, which until now resolved
            // to nothing because the folder didn't have it.
            broadcast_avatar(&state);
            Json(model).into_response()
        }
        Err(err) => error_response(vrm_error_status(&err), err.to_string()),
    }
}

/// `DELETE /api/vrm/:file`. Characters pointing at the deleted model are
/// deliberately left alone: the avatar reference is by name, so restoring
/// the file restores the avatar, and a dangling one already renders as the
/// plain initial rather than an error.
pub async fn api_delete_vrm(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    match npc_core::delete_vrm(&state.ctx.data_dir, &file) {
        Ok(true) => {
            // If this was the model on screen, the avatar has just become
            // nothing — tabs showing it need to fall back now, not on reload.
            broadcast_avatar(&state);
            Json(json!({ "ok": true })).into_response()
        }
        Ok(false) => error_response(StatusCode::NOT_FOUND, format!("vrm not found: {file}")),
        Err(err) => error_response(vrm_error_status(&err), err.to_string()),
    }
}

/// Body for `PUT /api/characters/:id/avatar`. A `file` of `null` (or an
/// empty string) clears the avatar — that's how the UI's "解除" acts, so it
/// doesn't need a second endpoint.
#[derive(serde::Deserialize)]
struct SetAvatarRequest {
    #[serde(default)]
    file: Option<String>,
}

/// `PUT /api/characters/:id/avatar`: point a character at a model in the
/// folder (or clear the pointer). The model must already be in the library —
/// assigning a name that isn't there would store a reference that can never
/// resolve, which is worth rejecting at the point the user can still fix it.
pub async fn api_set_character_avatar(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: String,
) -> Response {
    let req: SetAvatarRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, format!("invalid JSON: {err}")),
    };

    let mut character = match npc_core::load_character(&state.ctx.data_dir, &id) {
        Ok(c) => c,
        Err(err) => return error_response(StatusCode::NOT_FOUND, err.to_string()),
    };

    let file = req.file.unwrap_or_default();
    let file = file.trim();
    if file.is_empty() {
        character.avatar = None;
    } else {
        match npc_core::vrm_exists(&state.ctx.data_dir, file) {
            Ok(true) => {}
            Ok(false) => return error_response(StatusCode::NOT_FOUND, format!("vrm not found: {file}")),
            Err(err) => return error_response(vrm_error_status(&err), err.to_string()),
        }
        character.avatar = Some(npc_core::Avatar::vrm(file));
    }

    if let Err(err) = npc_core::save_character(&state.ctx.data_dir, &character) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string());
    }
    broadcast_avatar(&state);
    Json(character_json(&character)).into_response()
}

// ---------------------------------------------------------------------
// GET /api/memory
// ---------------------------------------------------------------------

/// Same file name as `npc_memory::STORE_FILE_NAME` (that module is private,
/// so this is a standalone constant rather than a shared one).
const MEMORY_STORE_FILE_NAME: &str = "memory-store.json";
/// Contract cap on `GET /api/memory`'s `longTerm` array.
const MAX_LONG_TERM_DOCS: usize = 200;

/// Cap on `GET /api/people/:id`'s `memories` array (contract §4).
const MAX_PERSON_MEMORIES: usize = 50;

/// Mirrors the on-disk shape of `npc_memory::store::Record` just enough to
/// read `doc_id`/`text`/`created_at`/`metadata` back out — `hash` and
/// `embedding` are present in the file but deliberately not modeled here, so
/// they're dropped on deserialize instead of ever reaching the response
/// (the embedding vector in particular must never be sent to the browser).
#[derive(Debug, Clone, serde::Deserialize)]
struct MemoryRecord {
    doc_id: String,
    text: String,
    /// Unix timestamp (seconds), per `npc_memory::store::Record::created_at`.
    created_at: i64,
    /// String-keyed metadata as written by `npc_memory` (e.g. `"type"`,
    /// `"timestamp"`, and — once a chunk has been attributed to someone —
    /// `"person_id"`). Missing/absent in older records, hence `#[default]`.
    #[serde(default)]
    metadata: std::collections::HashMap<String, String>,
}

impl MemoryRecord {
    fn person_id(&self) -> Option<&str> {
        self.metadata.get("person_id").map(String::as_str)
    }
}

fn format_created_at(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// `GET /api/memory`: the current short-term summary plus the long-term
/// vector store's records (text only — embeddings are never included).
/// Always 200; a missing/unreadable store is reported as empty rather than
/// as an error, since "no memories yet" is a normal, expected state.
pub async fn api_memory(State(state): State<AppState>) -> Response {
    let short_term = state.short_term_memory.lock().unwrap().clone();
    let mut records = read_long_term_records(&state.ctx.data_dir);
    records.truncate(MAX_LONG_TERM_DOCS);
    let long_term: Vec<Value> = records.into_iter().map(long_term_record_to_wire).collect();
    Json(json!({ "shortTerm": short_term, "longTerm": long_term })).into_response()
}

fn long_term_record_to_wire(r: MemoryRecord) -> Value {
    json!({
        "docId": r.doc_id,
        "text": r.text,
        "createdAt": format_created_at(r.created_at),
        "personId": r.person_id().unwrap_or_default(),
    })
}

/// Read every record from `{data_dir}/memory-store.json`, newest first. A
/// missing or unparseable store yields an empty list rather than an error
/// (matches `load_long_term_memory`'s previous "no memories yet is normal"
/// behavior) — callers apply their own cap/filter on top.
fn read_long_term_records(data_dir: &std::path::Path) -> Vec<MemoryRecord> {
    let path = data_dir.join(MEMORY_STORE_FILE_NAME);
    let data = match std::fs::read_to_string(&path) {
        Ok(data) => data,
        Err(_) => return Vec::new(),
    };
    let mut records: Vec<MemoryRecord> = match serde_json::from_str(&data) {
        Ok(records) => records,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "npc-server: failed to parse memory store, returning empty"
            );
            return Vec::new();
        }
    };
    records.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    records
}

/// Keep only the records attributed to `person_id`, capped to `limit`.
/// `records` is assumed already newest-first (as `read_long_term_records`
/// returns), so the result stays newest-first too.
fn filter_person_memories(records: Vec<MemoryRecord>, person_id: &str, limit: usize) -> Vec<MemoryRecord> {
    records
        .into_iter()
        .filter(|r| r.person_id() == Some(person_id))
        .take(limit)
        .collect()
}

/// `GET /api/people/:id`'s `memories` array: long-term store records whose
/// `metadata.person_id` matches `id`, newest first, capped at
/// [`MAX_PERSON_MEMORIES`]. Embeddings are never included (`MemoryRecord`
/// doesn't even model that field).
fn person_memories(data_dir: &std::path::Path, person_id: &str) -> Vec<Value> {
    let records = read_long_term_records(data_dir);
    filter_person_memories(records, person_id, MAX_PERSON_MEMORIES)
        .into_iter()
        .map(|r| json!({ "docId": r.doc_id, "text": r.text, "createdAt": format_created_at(r.created_at) }))
        .collect()
}

// ---------------------------------------------------------------------
// GET /api/affect/history
// ---------------------------------------------------------------------

/// Select the most recent `limit` entries from `history` (already
/// oldest-first), preserving that ordering. A `limit` at or beyond the
/// history's own length returns the whole thing unchanged — this is what
/// makes an *omitted* query parameter behave as "everything currently held"
/// once `api_affect_history` defaults it to the buffer's own cap.
fn recent_affect_entries(
    history: &std::collections::VecDeque<crate::bus_forward::AffectHistoryEntry>,
    limit: usize,
) -> Vec<crate::bus_forward::AffectHistoryEntry> {
    let skip = history.len().saturating_sub(limit);
    history.iter().skip(skip).cloned().collect()
}

/// `GET /api/affect/history?limit=N`: the server's rolling buffer of recent
/// `affect` snapshots (see `bus_forward::AFFECT_HISTORY_CAP`), returned
/// oldest-first / newest-last — the same order `GET /api/chat/history`
/// promises, and for the same reason: the caller (the 感情 tab's sparkline)
/// wants to append these in front of its live frames, not sort them itself.
///
/// This endpoint exists because the browser's own affect history
/// (`useNpcSocket`'s `affectHistory`) is filled only by live `affect` WS
/// frames and starts empty on every page load — the trend line the 感情 tab
/// exists to show would otherwise stay blank until the NPC spoke again.
///
/// `limit` defaults to, and is clamped to, `bus_forward::AFFECT_HISTORY_CAP`:
/// the buffer itself never holds more entries than that, so a larger
/// requested limit could never be satisfied anyway and an omitted one simply
/// means "everything you have."
///
/// Always 200: an empty buffer (fresh process, nobody has spoken yet) is a
/// normal state, not an error — matching `api_memory`'s contract.
pub async fn api_affect_history(State(state): State<AppState>, Query(params): Query<HashMap<String, String>>) -> Response {
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(crate::bus_forward::AFFECT_HISTORY_CAP)
        .min(crate::bus_forward::AFFECT_HISTORY_CAP);

    let history = state.affect_history.lock().unwrap();
    let entries = recent_affect_entries(&history, limit);
    Json(json!({ "entries": entries })).into_response()
}

// ---------------------------------------------------------------------
// /api/people
// ---------------------------------------------------------------------

/// Publish `person_updated` on `topic::UI` so `bus_forward` relays it to
/// every connected WS client as a `person` frame (contract §4/§2).
fn publish_person_updated(state: &AppState, person: &npc_core::Person) {
    state
        .ctx
        .bus
        .publish(npc_core::topic::UI, npc_core::msg::PERSON_UPDATED, npc_core::person_to_wire(person));
}

/// Publish `person_deleted` on `topic::UI` so `bus_forward` relays it as a
/// `personDeleted` frame.
fn publish_person_deleted(state: &AppState, id: &str) {
    state
        .ctx
        .bus
        .publish(npc_core::topic::UI, npc_core::msg::PERSON_DELETED, json!({ "id": id }));
}

/// `npc_core::load_person`/`delete_person` return `Err` both for a
/// syntactically invalid id (the path-traversal guard) and for a genuine
/// I/O/parse failure on an otherwise valid id. The former is a client
/// mistake — a crafted or corrupted URL — so it must never surface as a
/// 5xx; the latter is a real server-side fault. This tells them apart from
/// the error message `npc_core::person::validate_person_id` produces.
fn person_error_status(err: &anyhow::Error) -> StatusCode {
    if err.to_string().contains("invalid person id") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

/// Newest-`lastSeen`-first ordering for `GET /api/people` (contract §4).
fn sort_people_by_last_seen_desc(people: &mut [npc_core::Person]) {
    people.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
}

pub async fn api_list_people(State(state): State<AppState>) -> Response {
    match npc_core::list_people(&state.ctx.data_dir) {
        Ok(mut people) => {
            sort_people_by_last_seen_desc(&mut people);
            let out: Vec<Value> = people.iter().map(npc_core::person_to_wire).collect();
            Json(json!({ "people": out })).into_response()
        }
        Err(err) => error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    }
}

/// Body for `POST /api/people`.
#[derive(serde::Deserialize)]
struct CreatePersonRequest {
    name: String,
    #[serde(default)]
    notes: Option<String>,
}

/// Trim `name` and reject it (`Err`) if that leaves nothing — a bare space
/// or empty string isn't a usable person name.
fn validate_person_name(name: &str) -> Result<&str, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        Err("name is required".to_string())
    } else {
        Ok(trimmed)
    }
}

pub async fn api_create_person(State(state): State<AppState>, body: String) -> Response {
    let req: CreatePersonRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, format!("invalid JSON: {err}")),
    };
    let name = match validate_person_name(&req.name) {
        Ok(n) => n,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, err),
    };

    let people = match npc_core::list_people(&state.ctx.data_dir) {
        Ok(p) => p,
        Err(err) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()),
    };
    // Don't create a second record for someone who already has one — hand
    // back the existing person instead (no bus publish: nothing changed).
    if let Some(existing) = npc_core::find_person(&people, name) {
        return Json(json!({ "person": npc_core::person_to_wire(existing) })).into_response();
    }

    let mut person = npc_core::new_person(name, "manual");
    if let Some(notes) = req.notes {
        person.notes = notes;
    }
    if let Err(err) = npc_core::save_person(&state.ctx.data_dir, &person) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string());
    }
    publish_person_updated(&state, &person);
    Json(json!({ "person": npc_core::person_to_wire(&person) })).into_response()
}

pub async fn api_get_person(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let person = match npc_core::load_person(&state.ctx.data_dir, &id) {
        Ok(Some(p)) => p,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, format!("person not found: {id}")),
        Err(err) => return error_response(person_error_status(&err), err.to_string()),
    };
    let memories = person_memories(&state.ctx.data_dir, &id);
    Json(json!({ "person": npc_core::person_to_wire(&person), "memories": memories })).into_response()
}

/// Body for `PATCH /api/people/:id`. Every field is optional — only the
/// ones present in the request are applied, everything else keeps its
/// current value (see [`apply_person_update`]).
#[derive(Debug, Default, serde::Deserialize)]
struct UpdatePersonRequest {
    #[serde(default)]
    name: Option<String>,
    /// Replaces the whole `aliases` array when present (not merged).
    #[serde(default)]
    aliases: Option<Vec<String>>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    appearance: Option<String>,
}

/// Apply only the fields `req` actually sent onto `person`, leaving the rest
/// untouched.
fn apply_person_update(person: &mut npc_core::Person, req: &UpdatePersonRequest) {
    if let Some(name) = &req.name {
        person.name = name.clone();
    }
    if let Some(aliases) = &req.aliases {
        person.aliases = aliases.clone();
    }
    if let Some(notes) = &req.notes {
        person.notes = notes.clone();
    }
    if let Some(appearance) = &req.appearance {
        person.appearance = appearance.clone();
    }
}

pub async fn api_update_person(State(state): State<AppState>, Path(id): Path<String>, body: String) -> Response {
    let req: UpdatePersonRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(err) => return error_response(StatusCode::BAD_REQUEST, format!("invalid JSON: {err}")),
    };
    let mut person = match npc_core::load_person(&state.ctx.data_dir, &id) {
        Ok(Some(p)) => p,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, format!("person not found: {id}")),
        Err(err) => return error_response(person_error_status(&err), err.to_string()),
    };
    apply_person_update(&mut person, &req);
    if let Err(err) = npc_core::save_person(&state.ctx.data_dir, &person) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, err.to_string());
    }
    publish_person_updated(&state, &person);
    Json(json!({ "person": npc_core::person_to_wire(&person) })).into_response()
}

pub async fn api_delete_person(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match npc_core::delete_person(&state.ctx.data_dir, &id) {
        Ok(true) => {
            publish_person_deleted(&state, &id);
            Json(json!({ "ok": true })).into_response()
        }
        Ok(false) => error_response(StatusCode::NOT_FOUND, format!("person not found: {id}")),
        Err(err) => error_response(person_error_status(&err), err.to_string()),
    }
}

// ---------------------------------------------------------------------
// GET /api/chat/history
// ---------------------------------------------------------------------

/// Default value of `GET /api/chat/history`'s `limit` query parameter, when
/// omitted or unparseable.
const CHAT_HISTORY_DEFAULT_LIMIT: usize = 200;
/// Upper bound `limit` is clamped to, regardless of what the client asks for.
const CHAT_HISTORY_MAX_LIMIT: usize = 1000;

/// Build the wire form of one chat log entry, dropping `speaker` entirely
/// when absent rather than emitting `"speaker": null` — done by hand here
/// (instead of relying on `ChatLogEntry`'s own `Serialize` impl) so this
/// endpoint's contract doesn't silently depend on a `skip_serializing_if`
/// attribute living in `npc-core`.
fn chat_log_entry_to_wire(entry: &npc_core::chatlog::ChatLogEntry) -> Value {
    let mut value = json!({
        "time": entry.time,
        "input": entry.input,
        "output": entry.output,
    });
    if let Some(speaker) = &entry.speaker {
        value["speaker"] = json!(speaker);
    }
    value
}

/// `GET /api/chat/history?limit=N`: the most recent chat turns from the
/// on-disk chat log, returned oldest-first (chronological order) so the UI
/// can append them straight into a scrolling transcript. `limit` defaults to
/// [`CHAT_HISTORY_DEFAULT_LIMIT`] and is clamped to [`CHAT_HISTORY_MAX_LIMIT`].
/// Sorted here by `time` (rather than trusting `load_recent`'s own ordering)
/// so the oldest-first contract holds regardless of how that function
/// internally orders its result.
///
/// `load_recent` does blocking file I/O and the log can grow to several
/// thousand lines, so it runs on `spawn_blocking` rather than inline on this
/// async handler's Tokio worker thread. A `JoinError` (only possible if that
/// blocking task panics) is logged and treated as "no entries" so this
/// endpoint keeps its always-200 contract.
pub async fn api_chat_history(State(state): State<AppState>, Query(params): Query<HashMap<String, String>>) -> Response {
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(CHAT_HISTORY_DEFAULT_LIMIT)
        .min(CHAT_HISTORY_MAX_LIMIT);

    let data_dir = state.ctx.data_dir.clone();
    let mut entries = match tokio::task::spawn_blocking(move || npc_core::chatlog::load_recent(&data_dir, limit)).await {
        Ok(entries) => entries,
        Err(err) => {
            tracing::error!(error = %err, "npc-server: chat log load task panicked, returning empty");
            Vec::new()
        }
    };
    entries.sort_by(|a, b| a.time.cmp(&b.time));

    let out: Vec<Value> = entries.iter().map(chat_log_entry_to_wire).collect();
    Json(json!({ "entries": out })).into_response()
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

        assert_eq!(resolve_probe_api_key(None, Some("api"), "***", &config), "sk-api");
        assert_eq!(resolve_probe_api_key(None, Some("tts"), "***", &config), "sk-tts");
        assert_eq!(resolve_probe_api_key(None, Some("stt"), "***", &config), "sk-stt");
    }

    #[test]
    fn resolve_probe_api_key_defaults_section_to_api_when_omitted() {
        let mut config = npc_core::Config::default();
        config.api.api_key = "sk-api".to_string();
        assert_eq!(resolve_probe_api_key(None, None, "***", &config), "sk-api");
    }

    #[test]
    fn resolve_probe_api_key_prefers_provider_id_over_section() {
        let mut config = npc_core::Config::default();
        config.api.api_key = "sk-api".to_string();
        config.providers.push(npc_core::config::ProviderConfig {
            id: "p1".to_string(),
            label: "P1".to_string(),
            base_url: "http://p1".to_string(),
            api_key: "sk-provider".to_string(),
        });
        assert_eq!(resolve_probe_api_key(Some("p1"), Some("api"), "***", &config), "sk-provider");
    }

    #[test]
    fn resolve_probe_api_key_falls_back_to_section_when_provider_id_unknown() {
        let mut config = npc_core::Config::default();
        config.tts.api_key = "sk-tts".to_string();
        assert_eq!(resolve_probe_api_key(Some("missing"), Some("tts"), "***", &config), "sk-tts");
    }

    #[test]
    fn resolve_probe_api_key_passes_through_typed_key() {
        let config = npc_core::Config::default();
        assert_eq!(
            resolve_probe_api_key(None, Some("api"), "sk-typed-just-now", &config),
            "sk-typed-just-now"
        );
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
        assert_eq!(req.section, Some("tts".to_string()));
        assert_eq!(req.provider_id, None);
    }

    #[test]
    fn parse_llm_probe_request_parses_provider_id_and_omitted_section() {
        let req = parse_llm_probe_request(
            r#"{"baseUrl": "http://localhost:11434", "apiKey": "***", "providerId": "p1"}"#,
        )
        .unwrap();
        assert_eq!(req.provider_id, Some("p1".to_string()));
        assert_eq!(req.section, None);
    }

    // -------------------------------------------------------------
    // /api/people
    // -------------------------------------------------------------

    fn record(doc_id: &str, created_at: i64, person_id: Option<&str>) -> MemoryRecord {
        let mut metadata = std::collections::HashMap::new();
        if let Some(pid) = person_id {
            metadata.insert("person_id".to_string(), pid.to_string());
        }
        MemoryRecord {
            doc_id: doc_id.to_string(),
            text: format!("text-{doc_id}"),
            created_at,
            metadata,
        }
    }

    #[test]
    fn person_error_status_maps_invalid_id_to_bad_request_not_5xx() {
        let err = anyhow::anyhow!("invalid person id: \"../etc\"");
        assert_eq!(person_error_status(&err), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn person_error_status_maps_other_errors_to_internal_server_error() {
        let err = anyhow::anyhow!("failed to parse person abc: unexpected EOF");
        assert_eq!(person_error_status(&err), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn sort_people_by_last_seen_desc_orders_newest_first() {
        let mut a = npc_core::new_person("太郎", "manual");
        a.last_seen = 100;
        let mut b = npc_core::new_person("花子", "manual");
        b.last_seen = 300;
        let mut c = npc_core::new_person("次郎", "manual");
        c.last_seen = 200;
        let mut people = vec![a.clone(), b.clone(), c.clone()];

        sort_people_by_last_seen_desc(&mut people);
        assert_eq!(people.iter().map(|p| p.id.clone()).collect::<Vec<_>>(), vec![b.id, c.id, a.id]);
    }

    #[test]
    fn validate_person_name_rejects_blank_and_trims() {
        assert!(validate_person_name("").is_err());
        assert!(validate_person_name("   ").is_err());
        assert_eq!(validate_person_name("  太郎  ").unwrap(), "太郎");
    }

    #[test]
    fn apply_person_update_only_touches_fields_that_were_sent() {
        let mut person = npc_core::new_person("太郎", "manual");
        person.notes = "original notes".to_string();
        person.appearance = "original look".to_string();

        // Only `notes` is present in the request.
        let req = UpdatePersonRequest {
            name: None,
            aliases: None,
            notes: Some("updated notes".to_string()),
            appearance: None,
        };
        apply_person_update(&mut person, &req);

        assert_eq!(person.name, "太郎"); // untouched
        assert_eq!(person.notes, "updated notes"); // updated
        assert_eq!(person.appearance, "original look"); // untouched
    }

    #[test]
    fn apply_person_update_replaces_aliases_array_wholesale() {
        let mut person = npc_core::new_person("太郎", "manual");
        person.aliases = vec!["old-alias".to_string()];

        let req = UpdatePersonRequest {
            name: None,
            aliases: Some(vec!["new-alias-1".to_string(), "new-alias-2".to_string()]),
            notes: None,
            appearance: None,
        };
        apply_person_update(&mut person, &req);

        assert_eq!(person.aliases, vec!["new-alias-1".to_string(), "new-alias-2".to_string()]);
    }

    #[test]
    fn apply_person_update_with_no_fields_is_a_no_op() {
        let mut person = npc_core::new_person("太郎", "manual");
        let before = format!("{person:?}");
        apply_person_update(&mut person, &UpdatePersonRequest::default());
        assert_eq!(format!("{person:?}"), before);
    }

    #[test]
    fn filter_person_memories_keeps_only_matching_person_and_respects_limit() {
        let records = vec![
            record("1", 300, Some("person-a")),
            record("2", 200, Some("person-b")),
            record("3", 100, Some("person-a")),
            record("4", 50, None),
        ];

        let filtered = filter_person_memories(records, "person-a", 50);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].doc_id, "1");
        assert_eq!(filtered[1].doc_id, "3");
    }

    #[test]
    fn filter_person_memories_respects_limit_below_match_count() {
        let records = vec![record("1", 300, Some("p")), record("2", 200, Some("p")), record("3", 100, Some("p"))];
        let filtered = filter_person_memories(records, "p", 2);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].doc_id, "1");
        assert_eq!(filtered[1].doc_id, "2");
    }

    #[test]
    fn filter_person_memories_no_match_returns_empty() {
        let records = vec![record("1", 100, Some("someone-else"))];
        assert!(filter_person_memories(records, "person-a", 50).is_empty());
    }

    #[test]
    fn long_term_record_to_wire_defaults_missing_person_id_to_empty_string() {
        let wire = long_term_record_to_wire(record("1", 100, None));
        assert_eq!(wire["personId"], "");
        assert_eq!(wire["docId"], "1");
    }

    #[test]
    fn long_term_record_to_wire_includes_person_id_when_present() {
        let wire = long_term_record_to_wire(record("1", 100, Some("person-a")));
        assert_eq!(wire["personId"], "person-a");
    }

    // -------------------------------------------------------------
    // /api/chat/history
    // -------------------------------------------------------------

    fn chat_entry(time: &str, speaker: Option<&str>, input: &str, output: &str) -> npc_core::chatlog::ChatLogEntry {
        npc_core::chatlog::ChatLogEntry {
            time: time.to_string(),
            speaker: speaker.map(str::to_string),
            input: input.to_string(),
            output: output.to_string(),
        }
    }

    #[test]
    fn chat_log_entry_to_wire_omits_speaker_field_when_absent() {
        let entry = chat_entry("2026-07-26T00:00:00Z", None, "hi", "hello");
        let wire = chat_log_entry_to_wire(&entry);
        assert!(wire.get("speaker").is_none());
        assert_eq!(wire["input"], "hi");
        assert_eq!(wire["output"], "hello");
    }

    #[test]
    fn chat_log_entry_to_wire_includes_speaker_when_present() {
        let entry = chat_entry("2026-07-26T00:00:00Z", Some("太郎"), "hi", "hello");
        let wire = chat_log_entry_to_wire(&entry);
        assert_eq!(wire["speaker"], "太郎");
    }

    // -------------------------------------------------------------
    // /api/affect/history
    // -------------------------------------------------------------

    fn affect_entry(ts: i64) -> crate::bus_forward::AffectHistoryEntry {
        crate::bus_forward::AffectHistoryEntry {
            ts,
            familiarity: 0.5,
            closing: false,
            invite_caution: false,
            partner: None,
            partner_known: false,
            partner_switched: false,
            partner_away: false,
            drives: Vec::new(),
        }
    }

    #[test]
    fn recent_affect_entries_returns_everything_when_limit_covers_all() {
        let history: std::collections::VecDeque<_> = (1..=3).map(affect_entry).collect();
        let out = recent_affect_entries(&history, 10);
        assert_eq!(out.iter().map(|e| e.ts).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn recent_affect_entries_keeps_newest_tail_oldest_first() {
        let history: std::collections::VecDeque<_> = (1..=5).map(affect_entry).collect();
        let out = recent_affect_entries(&history, 2);
        // Entries 4 and 5 are the most recent, and still appear oldest-first.
        assert_eq!(out.iter().map(|e| e.ts).collect::<Vec<_>>(), vec![4, 5]);
    }

    #[test]
    fn recent_affect_entries_limit_zero_returns_empty() {
        let history: std::collections::VecDeque<_> = (1..=3).map(affect_entry).collect();
        assert!(recent_affect_entries(&history, 0).is_empty());
    }

    #[test]
    fn recent_affect_entries_on_empty_history_returns_empty() {
        let history: std::collections::VecDeque<crate::bus_forward::AffectHistoryEntry> =
            std::collections::VecDeque::new();
        assert!(recent_affect_entries(&history, 60).is_empty());
    }
}
