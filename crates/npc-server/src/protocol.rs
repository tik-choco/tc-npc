//! WS frame shapes exchanged with the web UI (`web/src/lib/types.ts` is the
//! contract this module implements — field names and tag values must match
//! exactly).

use serde::{Deserialize, Serialize};

/// `Record<string, boolean>` on the TS side — plain struct fields serialize
/// to exactly that shape.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleFlags {
    pub talk: bool,
    pub memory: bool,
    pub speech: bool,
    pub vision: bool,
    pub action: bool,
    pub scheduler: bool,
    /// `config.translation.mode` is not `off` — i.e. the 通訳 tab is live.
    pub translation: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CharacterRef {
    pub id: String,
    pub name: String,
}

/// The avatar model to display, as sent to the browser. `file` names an
/// entry in the local model folder, fetched over `GET /api/vrm/file/:file`.
///
/// Deliberately a sibling of `character` rather than a field on it: which
/// model is on screen is a display concern, and tc-npc chats perfectly well
/// with no character sheet loaded. Requiring one just to show an avatar
/// would mean importing a tc-town export before a VRM could be used at all.
/// So this resolves to the active character's own avatar when there is one,
/// and otherwise to `config.character.avatar_file`.
#[derive(Debug, Clone, Serialize)]
pub struct AvatarRef {
    pub kind: String,
    pub file: String,
}

impl From<npc_core::Avatar> for AvatarRef {
    fn from(a: npc_core::Avatar) -> Self {
        AvatarRef {
            kind: a.kind,
            file: a.file,
        }
    }
}

/// One drive's current reading within an `affect` frame's `drives` array.
/// `key` is the contract's snake_case wire key (see
/// `npc_talk::affect::DriveKey::as_str`), e.g. `"substance_p"`.
#[derive(Debug, Clone, Serialize)]
pub struct DriveLevel {
    pub key: String,
    pub level: f32,
    pub base: f32,
}

/// Server -> client frames. Internally tagged on `"type"`, matching
/// `ServerMessage` in `web/src/lib/types.ts`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    #[serde(rename = "hello")]
    Hello {
        version: String,
        modules: ModuleFlags,
        character: Option<CharacterRef>,
        /// The model to display, resolved per [`AvatarRef`]. Sent in `hello`
        /// so the (large) model can start loading before any REST call.
        #[serde(skip_serializing_if = "Option::is_none")]
        avatar: Option<AvatarRef>,
    },
    #[serde(rename = "chat")]
    Chat { role: String, text: String, ts: i64 },
    /// npc-talk took a turn without speaking (`agent:chat` / `chat_silent`):
    /// the conversation is over (`reason: "closing"`) or the NPC declined to
    /// answer this utterance (`"declined"`). Carries no text — the point is
    /// that there isn't any — and stands in for the `chat` frame that would
    /// otherwise end the turn.
    #[serde(rename = "silent")]
    Silent { reason: String, ts: i64 },
    #[serde(rename = "ttsLine")]
    TtsLine {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        translations: Option<serde_json::Value>,
    },
    #[serde(rename = "sense")]
    Sense { kind: String, text: String, ts: i64 },
    /// One simultaneous-interpretation update from npc-translate. `lang` is
    /// empty for the "heard this line, translations pending" frame and names
    /// the target language for each finished translation; all frames for one
    /// utterance share an `id` so the client groups them.
    #[serde(rename = "translation")]
    Translation {
        id: String,
        source: String,
        original: String,
        lang: String,
        text: String,
        reversed: bool,
        ts: i64,
    },
    #[serde(rename = "memory")]
    Memory { kind: String, text: String },
    /// The NPC's internal affect/drive state, published by npc-talk after
    /// every conversation turn (see `npc_talk::affect::PartnerAffect::snapshot`).
    /// The drive state is per conversation partner: `partner` names who it
    /// belongs to (absent while nobody has identified themselves), and
    /// `partnerSwitched` marks the single frame on which npc-talk detected
    /// the partner changing.
    #[serde(rename = "affect")]
    Affect {
        ts: i64,
        familiarity: f32,
        closing: bool,
        #[serde(rename = "inviteCaution")]
        invite_caution: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        partner: Option<String>,
        #[serde(rename = "partnerKnown")]
        partner_known: bool,
        #[serde(rename = "partnerSwitched")]
        partner_switched: bool,
        /// The partner has been silent past `talk.affect.absence_timeout_secs`
        /// and is treated as having left. Stays set until somebody speaks.
        #[serde(rename = "partnerAway")]
        partner_away: bool,
        drives: Vec<DriveLevel>,
    },
    #[serde(rename = "actionLog")]
    ActionLog { text: String },
    #[serde(rename = "position")]
    Position { x: f64, y: f64, heading: f64 },
    #[serde(rename = "volume")]
    Volume { level: f64 },
    /// The NPC's synthesized voice is (or is no longer) audible on the host's
    /// speakers, published by npc-speech's playback thread. Drives the VRM
    /// avatar's mouth: the browser is never sent the audio itself, so this is
    /// the only signal that tracks the real voice. Sent only on transitions.
    #[serde(rename = "speaking")]
    Speaking { active: bool },
    /// Continuous loudness (`0.0..=1.0`) of the clip `Speaking { active: true
    /// }` is currently announcing, sampled from npc-speech's playback
    /// position every ~50ms. `speaking` is the authoritative on/off edge —
    /// it is sent only on a real transition and controls whether the mouth
    /// animates at all — while this rides alongside it as the continuous
    /// signal the VRM mouth actually tracks, so it moves with the real
    /// envelope of the voice instead of a fixed sine wave for the clip's
    /// whole duration.
    #[serde(rename = "speakingLevel")]
    SpeakingLevel { level: f64 },
    /// Which model to display has changed — a VRM was assigned or cleared,
    /// or a different character was activated. Broadcast so a tab that was
    /// already open picks it up: `hello` carries the avatar only at connect
    /// time, so without this the チャット tab would keep its avatar layout
    /// disabled until a reload. `avatar` is null when there is none.
    #[serde(rename = "avatar")]
    Avatar { avatar: Option<AvatarRef> },
    /// Current state of the cascade voice loop's 開始/停止 switch. Sent to a
    /// client right after `hello` so its toggle starts in the right position,
    /// and broadcast on every change so every open tab agrees.
    #[serde(rename = "voice")]
    Voice { active: bool },
    #[allow(dead_code)]
    #[serde(rename = "status")]
    Status { modules: ModuleFlags },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "inputAccepted")]
    InputAccepted {
        #[serde(rename = "requestId")]
        request_id: String,
    },
    /// Immediate ack that a viewer/platform `event` was received, mirroring
    /// `inputAccepted`'s role for `input`. Carries `kind` back (not a
    /// requestId — events don't get a `response` frame of their own, they
    /// feed the same chat pipeline as an `input` under the hood) so an
    /// extension juggling several event types can tell which ack is which.
    #[serde(rename = "eventAccepted")]
    EventAccepted { kind: String },
    /// Immediate ack for `interrupt`. No fields: unlike `input`/`event` there
    /// is nothing to echo back, `interrupt` is a bare signal.
    #[serde(rename = "interruptAccepted")]
    InterruptAccepted,
    /// Immediate ack for `suspend`. Sent the instant the gate closes, not
    /// once the current turn (if any) finishes — an extension waiting on
    /// this to know its `suspend` was heard shouldn't have to wait out
    /// whatever reply is already in flight.
    #[serde(rename = "suspendAccepted")]
    SuspendAccepted,
    /// Immediate ack for `resume`. Sent before the queued `input`/`event`
    /// backlog (see `ws::drain_suspend_queue`) is worked through, so an
    /// extension polling for this doesn't have to wait for that replay too.
    #[serde(rename = "resumeAccepted")]
    ResumeAccepted,
    #[serde(rename = "response")]
    Response {
        #[serde(rename = "requestId")]
        request_id: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// A person record was created or updated. `person` is the camelCase
    /// `PersonRecord` shape produced by `npc_core::person_to_wire` (see the
    /// person-memory contract §5) — passed through as-is rather than
    /// re-modeled here since npc-server only forwards it.
    #[serde(rename = "person")]
    Person { person: serde_json::Value },
    /// A person record was deleted.
    #[serde(rename = "personDeleted")]
    PersonDeleted { id: String },
}

/// Client -> server frames. Internally tagged on `"type"`, matching
/// `ClientMessage` in `web/src/lib/types.ts`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMsg {
    #[serde(rename = "input")]
    Input {
        text: String,
        /// Optional display name of whoever typed `text`, so the bus
        /// message can be attributed to a person record (see the
        /// person-memory contract §4). Absent/`None` leaves the payload
        /// exactly as before (no `speaker` field).
        #[serde(default)]
        speaker: Option<String>,
    },
    #[serde(rename = "command")]
    Command { text: String },
    #[serde(rename = "interrupt")]
    Interrupt,
    #[serde(rename = "suspend")]
    Suspend,
    #[serde(rename = "resume")]
    Resume,
    /// Flip the cascade voice loop's master switch. Unlike `suspend`/`resume`
    /// (TTS-only, auto-expiring) this also controls whether the mic is fed to
    /// the VAD/STT pipeline, so the user can start and stop talking to the
    /// NPC from the チャット tab.
    #[serde(rename = "voiceStart")]
    VoiceStart,
    #[serde(rename = "voiceStop")]
    VoiceStop,
    #[serde(rename = "event")]
    Event {
        kind: String,
        #[serde(rename = "userName", default)]
        user_name: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        amount: Option<f64>,
        /// Subscription tier (`subscribe`/`resub`/`gift`) — see the extension
        /// API spec's "`event` の `kind` 別フィールド" table.
        #[serde(default)]
        tier: Option<String>,
        /// Free-text attached to the event: a `resub`/`cheer` comment, or a
        /// channel-points redemption's user input.
        #[serde(default)]
        message: Option<String>,
        /// The redeemed reward's name (`points` only), serde-renamed from
        /// camelCase like `userName` above.
        #[serde(rename = "rewardTitle", default)]
        reward_title: Option<String>,
    },
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
