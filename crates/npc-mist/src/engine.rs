//! Thin wrapper around mistlib-native's process-wide singleton engine
//! (`mistlib::app::*`), mirroring the embedding pattern the `mistl` daemon
//! app uses (see `mistl/src/net/mod.rs`), simplified for tc-npc's
//! single-room use.
//!
//! IMPORTANT: as of the pinned mistlib tag (v0.6.0), mistlib-native's
//! engine tracks exactly **one** joined room at a time --
//! `mistlib-native/src/transports/webrtc.rs`'s `WebRtcTransport::room_id`
//! is a plain `RwLock<String>` that `join_room`/`set_room_id` *overwrite*,
//! not a set that grows. Newer mistlib builds may support additive
//! multi-room engines, but the public pinned tag does not, so `lib.rs`
//! picks a single room to join accordingly (presence room XOR catalog
//! room).

use std::sync::OnceLock;

use anyhow::Context;
use tokio::sync::mpsc;

/// mistlib's registered event callback is a bare `unsafe extern "C" fn`
/// pointer with no captured state (`mistlib::events::EventCallback`) -- it
/// can't be a closure. So, like `mistl`'s own `dispatch_room_event`, the
/// actual handler state (this channel's sender) lives in a static instead.
static RAW_TX: OnceLock<mpsc::UnboundedSender<Vec<u8>>> = OnceLock::new();

/// mistlib's dispatch thread calls this synchronously, with no tokio
/// runtime of its own available -- keep it cheap (just a channel send) and
/// hand real (async, fallible) processing off to our own async world via
/// `RAW_TX`. Only `EVENT_RAW` (actual room messages) are forwarded; join/
/// leave/overlay events are ignored.
///
/// # Safety
/// Called only by mistlib's own event dispatch
/// (`mistlib_native::events::dispatch_event`), which guarantees `data_ptr`
/// is valid for `data_len` bytes for the duration of this call.
unsafe extern "C" fn dispatch_raw_event(
    event_type: u32,
    _from_ptr: *const u8,
    _from_len: usize,
    data_ptr: *const u8,
    data_len: usize,
) {
    if event_type != mistlib::events::EVENT_RAW {
        return;
    }
    let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) }.to_vec();
    if let Some(tx) = RAW_TX.get() {
        let _ = tx.send(data);
    }
}

/// Initializes the process-wide mistlib engine (identity + signaling) and
/// registers the raw event handler. Content storage is wired up
/// automatically *inside* mistlib-native's own `init()` ->
/// `build_context()` -> `init_storage()` (see
/// `mistlib-native/src/layers/native_l0/init.rs`), so no separate storage
/// setup call is needed here before [`storage_get`] can be used.
///
/// Must be called at most once per process -- returns an error on a second
/// call (`npc-mist` only ever runs one module instance, so this should
/// never happen in practice).
///
/// mistlib's sync entry points (`init`) call `ENGINE.runtime.block_on(...)`
/// internally, which panics if invoked directly from a tokio worker thread
/// (nested runtime entry) -- run on a blocking thread, mirroring how the
/// `mistl` daemon calls the same entry points (see its `net::start_engine`).
pub async fn start(
    node_id: String,
    signaling_url: String,
) -> anyhow::Result<mpsc::UnboundedReceiver<Vec<u8>>> {
    let (tx, rx) = mpsc::unbounded_channel();
    RAW_TX
        .set(tx)
        .map_err(|_| anyhow::anyhow!("mist engine already started in this process"))?;

    tokio::task::spawn_blocking(move || {
        mistlib::app::register_event_callback(dispatch_raw_event);
        mistlib::app::init(node_id, signaling_url);
    })
    .await
    .context("mist: initializing mistlib engine")?;

    Ok(rx)
}

/// Joins `room_id`, replacing whatever room (if any) was previously joined
/// -- see this module's doc comment on mistlib's single-room-per-engine
/// behavior at the pinned tag.
pub async fn join_room(room_id: String) -> anyhow::Result<()> {
    tokio::task::spawn_blocking(move || mistlib::app::join_room(room_id))
        .await
        .context("mist: joining room")
}

/// Leaves the currently joined room and tears down the WebRTC/WebSocket
/// session.
pub async fn leave_room() -> anyhow::Result<()> {
    tokio::task::spawn_blocking(mistlib::app::leave_room)
        .await
        .context("mist: leaving room")
}

/// Fetches content previously published via mist storage (`storage_add`),
/// e.g. tc-town's catalog payload JSON referenced by a `CatalogEntryWire`'s
/// `cid`. `NativeL0` is a zero-sized handle -- all real state lives in
/// mistlib's process-wide `ENGINE`/`STORAGE` statics, so constructing a
/// fresh one per call is fine (mirrors mistlib-native's own `app::*` free
/// functions, which do the same internally via `ENGINE.l0`).
pub async fn storage_get(cid: &str) -> anyhow::Result<Vec<u8>> {
    use mistlib_core::layers::L0Engine;
    mistlib::layers::NativeL0::new()
        .storage_get(cid)
        .await
        .map_err(|err| anyhow::anyhow!("mist: storage_get({cid}) failed: {err}"))
}
