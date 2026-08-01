//! In-memory TUI state and the pure state-transition logic driving it.
//! Deliberately free of any ratatui/crossterm/tokio types: everything here
//! is a plain data transformation from an incoming [`AppEvent`] to the next
//! `App`, so [`App::handle_event`]'s branches can be exercised by a unit
//! test without standing up a terminal or a socket (see `ui.rs` for the only
//! part of this crate that actually touches the screen).

use std::time::{Duration, Instant};

use crate::protocol::{CharacterRef, ModuleFlags, ServerMsg};

/// How many rows [`App::chat`]/[`App::log`] keep before dropping the oldest.
/// A TUI session can run for a long time (SSH'd into a machine and left
/// open); this bounds memory the same way `web/src/hooks/useNpcSocket.ts`'s
/// `MAX_ENTRIES` bounds the browser's, without needing to match its exact
/// number — there is no shared contract on scrollback length between the two
/// clients, only the general principle that neither should grow forever.
const MAX_ROWS: usize = 500;

/// How many rows [`App::event_log`] keeps. Deliberately much smaller than
/// [`MAX_ROWS`]: the event log isn't the chat transcript or the sense/memory
/// trace, it's "what did the operator/connection do" (toggled a view,
/// reconnected, …), so it doesn't need hundreds of entries to stay useful —
/// this mirrors agent-speech's `HistoryLimit = 200` (`model.go`), chosen
/// there for the same kind of low-volume operational log.
const EVENT_LOG_LIMIT: usize = 200;

/// How long [`App::error`] stays visible before [`App::tick`] clears it.
/// Matches agent-speech's `ErrorTTL = 10 * time.Second` (`model.go`) — long
/// enough for an operator glancing at the screen to read it, short enough
/// that a stale error doesn't linger forever once whatever caused it has
/// passed.
const ERROR_TTL: Duration = Duration::from_secs(10);

/// Mirrors `web/src/lib/ws.ts`'s `ConnectionState`, plus `Reconnecting` to
/// distinguish "never connected yet" from "was connected, dropped, retrying"
/// in the status bar — the web UI doesn't need that distinction because its
/// status chip has no room for a fourth word, but a terminal status line
/// does, and "reconnecting" is a materially different thing to tell an
/// operator than "still trying the first time".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
}

impl ConnectionState {
    pub fn label(self) -> &'static str {
        match self {
            ConnectionState::Connecting => "接続中...",
            ConnectionState::Connected => "接続済み",
            ConnectionState::Reconnecting => "再接続中...",
            ConnectionState::Disconnected => "切断",
        }
    }
}

/// A one-word summary of "what's the system doing right now", derived from
/// connection/pending state for the top status bar. The terminal-UI
/// analogue of agent-speech's `systemActivity()` (`pipeline.go`) — but
/// unlike that function, this one stops at the semantics: agent-speech
/// returns a Lip Gloss color directly from `systemActivityColor()`, which
/// means "what counts as urgent" and "how urgent looks" are the same
/// decision. Here they're split: [`App::system_activity`] only decides
/// *which* member applies, and `ui.rs` (which does import ratatui/`theme.rs`)
/// is the only place a color gets chosen. That split is what lets this
/// module stay renderer-agnostic while still driving the status bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemActivity {
    /// Never connected, or connected and then dropped for good (not
    /// currently retrying).
    Offline,
    /// Dialing in — either the first attempt or a retry after a drop.
    Connecting,
    /// Connected, and the last `input` sent is still awaiting its reply.
    Waiting,
    /// Connected, nothing in flight.
    Idle,
}

impl SystemActivity {
    pub fn label(self) -> &'static str {
        match self {
            SystemActivity::Offline => "オフライン",
            SystemActivity::Connecting => "接続中",
            SystemActivity::Waiting => "応答待ち",
            SystemActivity::Idle => "アイドル",
        }
    }
}

/// Which of the two screens is showing. Mirrors agent-speech's `ViewState`
/// (`ViewStateMain` / `ViewStateConfig` in `model.go`): `Main` is the chat
/// transcript + log + input box, `Config` is the module status / settings
/// screen. Two members only — there's no third screen — so `toggled()` is a
/// plain flip rather than a cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewState {
    Main,
    Config,
}

impl ViewState {
    fn toggled(self) -> ViewState {
        match self {
            ViewState::Main => ViewState::Config,
            ViewState::Config => ViewState::Main,
        }
    }
}

/// Which panel has keyboard focus on the [`ViewState::Main`] screen: the
/// input box, the chat transcript, or the log panel. (v1 only had
/// input/log — see the crate's git history — but a screen split three ways
/// needs a third focus target, so `Chat` was added here rather than left
/// unreachable.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MainFocus {
    Input,
    Chat,
    Log,
}

impl MainFocus {
    fn next(self) -> MainFocus {
        match self {
            MainFocus::Input => MainFocus::Chat,
            MainFocus::Chat => MainFocus::Log,
            MainFocus::Log => MainFocus::Input,
        }
    }

    fn prev(self) -> MainFocus {
        match self {
            MainFocus::Input => MainFocus::Log,
            MainFocus::Log => MainFocus::Chat,
            MainFocus::Chat => MainFocus::Input,
        }
    }
}

/// Which panel has keyboard focus on the [`ViewState::Config`] screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFocus {
    Modules,
    Characters,
}

impl ConfigFocus {
    fn next(self) -> ConfigFocus {
        match self {
            ConfigFocus::Modules => ConfigFocus::Characters,
            ConfigFocus::Characters => ConfigFocus::Modules,
        }
    }

    fn prev(self) -> ConfigFocus {
        // Two variants, so backwards and forwards coincide. Spelled out
        // rather than delegating to `next` so that adding a third fails
        // loudly here instead of silently cycling the wrong way.
        match self {
            ConfigFocus::Modules => ConfigFocus::Characters,
            ConfigFocus::Characters => ConfigFocus::Modules,
        }
    }
}

/// One row of the Config screen's character list. Deliberately *not*
/// `api::CharacterSummary`: that type carries the REST response's shape
/// (including an avatar reference this screen has no use for) and lives in a
/// module that pulls in tokio. A plain mirror here is what keeps `app.rs`
/// free of I/O dependencies — the mapping happens once, in `lib.rs`, where
/// the fetch is actually driven from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharacterEntry {
    pub id: String,
    pub name: String,
    pub active: bool,
}

/// Keyboard focus, paired with the screen it belongs to. This is a sum of
/// [`MainFocus`] and [`ConfigFocus`] rather than two independent `App`
/// fields (`main_focus: MainFocus, config_focus: ConfigFocus`) on purpose:
/// two independent fields can drift — e.g. `view == Config` while a stale
/// `main_focus` is still sitting on `Chat` — and every reader (`ui.rs`'s
/// rendering, this module's own transitions) would have to remember to
/// ignore whichever field doesn't match the current screen. Folding them
/// into one enum makes "focus for a screen that isn't showing" a state that
/// doesn't type-check, not just a state nobody's supposed to produce.
/// agent-speech gets away with a single flat `focus int` (`FocusCount = 4`,
/// `model.go`) because all four of its focus targets belong to the same
/// (Config) screen; ratatui's screen split here needs the pairing this enum
/// gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Main(MainFocus),
    Config(ConfigFocus),
}

impl Focus {
    fn next(self) -> Focus {
        match self {
            Focus::Main(f) => Focus::Main(f.next()),
            Focus::Config(f) => Focus::Config(f.next()),
        }
    }

    fn prev(self) -> Focus {
        match self {
            Focus::Main(f) => Focus::Main(f.prev()),
            Focus::Config(f) => Focus::Config(f.prev()),
        }
    }
}

/// One row in the chat transcript. `Silent` (see
/// `npc_server::protocol::ServerMsg::Silent`'s doc) is kept as its own kind
/// rather than folded into `Chat` with empty text, so the renderer can show
/// "(黙って反応しませんでした)" instead of a blank assistant line.
#[derive(Debug, Clone)]
pub enum ChatRow {
    Chat { role: String, text: String },
    Silent { reason: String },
}

/// One row in the log/event panel: `sense`, `memory`, `actionLog`, and
/// unmodeled frame kinds (see [`ServerMsg::Unknown`]) all land here rather
/// than in the chat transcript, since none of them are a conversational
/// turn.
#[derive(Debug, Clone)]
pub struct LogRow {
    pub label: String,
    pub text: String,
}

/// Everything the UI needs to render one frame, and the sole target of every
/// state transition in this module.
pub struct App {
    pub connection: ConnectionState,
    pub version: Option<String>,
    pub modules: ModuleFlags,
    pub character: Option<CharacterRef>,
    pub chat: Vec<ChatRow>,
    pub log: Vec<LogRow>,
    pub input: String,
    /// Which screen is showing. Read by `ui.rs` to pick which layout to
    /// draw, and by this module's own transitions to decide what `Tab`/`Esc`
    /// do next.
    pub view: ViewState,
    pub focus: Focus,
    /// Rows scrolled up from the bottom of the chat panel; 0 means "pinned
    /// to the latest message". Kept separate from `log_scroll` because the
    /// two panels are independently sized and scrolled.
    pub chat_scroll: u16,
    pub log_scroll: u16,
    /// Set once the last `input` sent is still awaiting its `chat`/`silent`/
    /// `error`/`response` reply — drives a "..." indicator the same way
    /// `useNpcSocket`'s `pending` does for the web UI's typing indicator.
    pub pending: bool,
    pub should_quit: bool,
    /// Latest error/status line shown just above the input box (a connection
    /// drop, a server `error` frame, malformed input) — the TUI equivalent of
    /// the web UI's toast/error list, collapsed to "just the latest one"
    /// since there's no room in a terminal for a scrolling toast stack.
    ///
    /// This predates, and is intentionally left alongside, [`App::error`]:
    /// `status_line` is protocol-driven (only [`App::apply_server_msg`] sets
    /// it) and never expires on its own, while `error` is for operator-side
    /// actions on the Config screen (a failed toggle, say) and clears itself
    /// after [`ERROR_TTL`] via [`App::tick`]. `ui.rs` deciding how the two
    /// should coexist on screen (e.g. only showing one at a time) is an
    /// integration concern outside this module's pure state.
    pub status_line: Option<String>,
    /// Ring buffer of operator/connection-level events — "reconnected",
    /// "switched to the Config screen" — as opposed to [`App::log`], which
    /// holds server-pushed `sense`/`memory`/`actionLog` frames. Capped at
    /// [`EVENT_LOG_LIMIT`]; see [`App::add_event_log`].
    pub event_log: Vec<String>,

    /// Characters the server knows about, for the Config screen. Empty until
    /// something fetches them — this crate learns about characters over REST
    /// (`GET /api/characters`), not over the WebSocket, which only ever
    /// reports the *active* one.
    pub characters: Vec<CharacterEntry>,
    /// Cursor into [`App::characters`].
    pub character_cursor: usize,
    /// A short-lived confirmation of the last operator action ("設定画面を
    /// 表示しています"). Unlike `error`, this has no TTL: it simply stays
    /// until the next action replaces it or a caller clears it with
    /// [`App::clear_notice`] — there's no server frame or clock tied to it,
    /// so there's nothing for [`App::tick`] to sweep.
    pub notice: Option<String>,
    /// The current operator-facing error and when it expires. The `Instant`
    /// here is an *expiry* timestamp handed in by the caller (see
    /// [`App::set_error`]), never one this module reads from the system
    /// clock itself — [`App::tick`] only ever compares against a `now` its
    /// caller passes in. That's what keeps this module's tests
    /// deterministic: a test can advance "time" by constructing two
    /// `Instant`s an arbitrary `Duration` apart, instead of needing the
    /// suite to actually sleep for [`ERROR_TTL`].
    pub error: Option<(String, Instant)>,
}

impl Default for App {
    fn default() -> Self {
        App {
            connection: ConnectionState::Connecting,
            version: None,
            modules: ModuleFlags::default(),
            character: None,
            chat: Vec::new(),
            log: Vec::new(),
            input: String::new(),
            view: ViewState::Main,
            focus: Focus::Main(MainFocus::Input),
            chat_scroll: 0,
            log_scroll: 0,
            pending: false,
            should_quit: false,
            status_line: None,
            event_log: Vec::new(),
            characters: Vec::new(),
            character_cursor: 0,
            notice: None,
            error: None,
        }
    }
}

/// Unmodeled frame kinds the server emits continuously rather than on an
/// event. Matched by name because that is all [`ServerMsg::Unknown`] carries;
/// the list is short and adding to it is cheap, whereas the alternative —
/// modelling each of these properly just to throw it away — is not.
fn is_high_frequency(kind: &str) -> bool {
    matches!(
        kind,
        "volume" | "speakingLevel" | "speaking" | "affect" | "position"
    )
}

fn push_capped<T>(rows: &mut Vec<T>, row: T, max: usize) {
    rows.push(row);
    if rows.len() > max {
        // Drop from the front: `rows` is arrival-ordered oldest-first, same
        // convention as the web UI's `cap()` helper.
        let excess = rows.len() - max;
        rows.drain(0..excess);
    }
}

impl App {
    /// Apply one server frame, updating chat/log/status state. This is the
    /// terminal-UI analogue of `useNpcSocket.ts`'s `handleMessage` switch —
    /// same frame kinds, much narrower reactions, since there's no VRM
    /// avatar, sparkline, or people list to feed here.
    pub fn apply_server_msg(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::Hello {
                version,
                modules,
                character,
            } => {
                self.version = Some(version);
                self.modules = modules;
                self.character = character;
            }
            ServerMsg::Status { modules } => {
                self.modules = modules;
            }
            ServerMsg::Chat { role, text, .. } => {
                let is_assistant = role == "assistant";
                push_capped(&mut self.chat, ChatRow::Chat { role, text }, MAX_ROWS);
                if is_assistant {
                    self.pending = false;
                }
            }
            ServerMsg::Silent { reason, .. } => {
                push_capped(&mut self.chat, ChatRow::Silent { reason }, MAX_ROWS);
                self.pending = false;
            }
            ServerMsg::Sense { kind, text, .. } => {
                push_capped(
                    &mut self.log,
                    LogRow {
                        label: format!("sense:{kind}"),
                        text,
                    },
                    MAX_ROWS,
                );
            }
            ServerMsg::Memory { kind, text } => {
                push_capped(
                    &mut self.log,
                    LogRow {
                        label: format!("memory:{kind}"),
                        text,
                    },
                    MAX_ROWS,
                );
            }
            ServerMsg::ActionLog { text } => {
                push_capped(
                    &mut self.log,
                    LogRow {
                        label: "action".to_string(),
                        text,
                    },
                    MAX_ROWS,
                );
            }
            ServerMsg::Error { message } => {
                self.pending = false;
                self.status_line = Some(format!("error: {message}"));
                push_capped(
                    &mut self.log,
                    LogRow {
                        label: "error".to_string(),
                        text: message,
                    },
                    MAX_ROWS,
                );
            }
            ServerMsg::InputAccepted { .. } => {
                self.pending = true;
            }
            ServerMsg::Response {
                status,
                text,
                message,
                ..
            } => {
                self.pending = false;
                if status == "error" {
                    let text = message.unwrap_or_else(|| "unknown error".to_string());
                    self.status_line = Some(format!("error: {text}"));
                } else if let Some(text) = text {
                    push_capped(
                        &mut self.log,
                        LogRow {
                            label: "response".to_string(),
                            text,
                        },
                        MAX_ROWS,
                    );
                }
            }
            ServerMsg::Unknown(kind) if is_high_frequency(&kind) => {
                // Dropped outright, unlike every other unmodeled frame. These
                // are continuous telemetry — the server emits `volume` many
                // times a second while audio is running — so logging them
                // buries everything else within a second or two and the panel
                // becomes useless. Observed on a real server: the log was
                // nothing but a wall of `[unhandled] volume`.
                //
                // The cap alone doesn't save it: `MAX_ROWS` is reached almost
                // immediately, so a genuinely interesting frame is evicted
                // before anyone can read it.
            }
            ServerMsg::Unknown(kind) => {
                // Not an error and not silently dropped either — see the
                // protocol module's doc for why an unmodeled frame kind
                // isn't a bug, but it's still worth a one-line trace in the
                // log panel so "the TUI doesn't do anything with 感情/VRM
                // frames" is visible behavior, not a silent black hole.
                push_capped(
                    &mut self.log,
                    LogRow {
                        label: "unhandled".to_string(),
                        text: kind,
                    },
                    MAX_ROWS,
                );
            }
        }
    }

    /// Update the connection state, logging the transition as an event —
    /// "reconnected", "dropped", etc. are exactly the kind of thing
    /// [`App::event_log`] exists for (see its doc). Guarded on an actual
    /// change so e.g. redundant `Connecting` -> `Connecting` calls don't
    /// spam the log.
    pub fn set_connection(&mut self, state: ConnectionState) {
        if self.connection != state {
            self.add_event_log(format!("接続状態: {}", state.label()));
        }
        self.connection = state;
    }

    /// A semantic snapshot of "what's happening right now" for the status
    /// bar. See [`SystemActivity`]'s doc for why this returns an enum and
    /// not a color.
    pub fn system_activity(&self) -> SystemActivity {
        match self.connection {
            ConnectionState::Disconnected => SystemActivity::Offline,
            ConnectionState::Connecting | ConnectionState::Reconnecting => {
                SystemActivity::Connecting
            }
            ConnectionState::Connected => {
                if self.pending {
                    SystemActivity::Waiting
                } else {
                    SystemActivity::Idle
                }
            }
        }
    }

    /// Cycle focus forward within whatever screen is currently showing
    /// (`Tab`). Wraps at the end of that screen's focus list rather than
    /// spilling into the other screen's — switching screens is
    /// [`App::toggle_view`]'s job, not this one's.
    pub fn next_focus(&mut self) {
        self.focus = self.focus.next();
    }

    /// Cycle focus backward within the current screen (`Shift+Tab`).
    pub fn prev_focus(&mut self) {
        self.focus = self.focus.prev();
    }

    /// Flip between the Main and Config screens (`Tab` at the screen level —
    /// bound to a different key than [`App::next_focus`]/[`App::prev_focus`]
    /// in `ui.rs`, since both can't own the same keystroke). Resets focus to
    /// that screen's first target, mirroring agent-speech resetting its
    /// list selections on entry to `ViewStateConfig` (`main_keys.go`) rather
    /// than leaving a focus index from the other screen dangling.
    pub fn toggle_view(&mut self) {
        self.view = self.view.toggled();
        self.focus = match self.view {
            ViewState::Main => Focus::Main(MainFocus::Input),
            ViewState::Config => Focus::Config(ConfigFocus::Modules),
        };
        match self.view {
            ViewState::Main => {
                self.set_notice("メイン画面に戻りました");
                self.add_event_log("メイン画面に切り替え");
            }
            ViewState::Config => {
                self.set_notice("設定画面を表示しています");
                self.add_event_log("設定画面に切り替え");
            }
        }
    }

    /// `Esc`: back to Main from Config, a no-op on Main itself. Mirrors
    /// agent-speech's `case "esc","backspace": if state == Config { ... }`
    /// (`main_keys.go`) — Esc has one direction only, it never becomes
    /// "switch to Config" by symmetry.
    pub fn return_to_main(&mut self) {
        if self.view == ViewState::Config {
            self.view = ViewState::Main;
            self.focus = Focus::Main(MainFocus::Input);
            self.set_notice("メイン画面に戻りました");
            self.add_event_log("メイン画面に切り替え");
        }
    }

    /// Record an operator/connection-level event (see [`App::event_log`]'s
    /// doc for how this differs from `log`). Blank/whitespace-only messages
    /// are dropped rather than stored, the same way [`App::take_input_for_send`]
    /// refuses to send blank input — an empty event isn't an event.
    pub fn add_event_log(&mut self, message: impl Into<String>) {
        let message = message.into();
        let trimmed = message.trim();
        if trimmed.is_empty() {
            return;
        }
        push_capped(&mut self.event_log, trimmed.to_string(), EVENT_LOG_LIMIT);
    }

    /// Set the notice line shown for the last operator action. See
    /// [`App::notice`]'s doc for why this has no expiry.
    pub fn set_notice(&mut self, message: impl Into<String>) {
        self.notice = Some(message.into());
    }

    pub fn clear_notice(&mut self) {
        self.notice = None;
    }

    /// Set the current operator-facing error, expiring [`ERROR_TTL`] after
    /// `now`. `now` is supplied by the caller (typically `Instant::now()` at
    /// the call site in `ui.rs`/the event loop) rather than read from the
    /// system clock in here — see [`App::error`]'s doc for why that split
    /// matters for testability.
    pub fn set_error(&mut self, message: impl Into<String>, now: Instant) {
        self.error = Some((message.into(), now + ERROR_TTL));
    }

    /// Sweep TTL'd state. Called once per UI tick with the current time
    /// handed in by the caller — this module never calls `Instant::now()`
    /// itself, for the same reason [`App::set_error`] takes `now` as a
    /// parameter instead of reading the clock: a test can pass any two
    /// `Instant`s it likes (`t0` and `t0 + Duration::from_secs(11)`, say)
    /// to assert expiry without an actual `sleep`, keeping this module's
    /// tests as clock-independent as the other 12 already are.
    pub fn tick(&mut self, now: Instant) {
        if let Some((_, expires_at)) = self.error {
            if now >= expires_at {
                self.error = None;
            }
        }
    }

    /// `(module name, enabled)` pairs in `hello`/`status`'s wire order, for
    /// the Config screen's module list to iterate — reading this instead of
    /// destructuring [`ModuleFlags`] by hand means a new module added to the
    /// protocol only needs updating here, not at every call site that lists
    /// modules.
    /// Replace the character list (a fresh `GET /api/characters`). The cursor
    /// is clamped rather than reset: refetching after activating a character
    /// shouldn't yank the selection back to the top of the list under the
    /// user's hands.
    pub fn set_characters(&mut self, characters: Vec<CharacterEntry>) {
        self.characters = characters;
        let last = self.characters.len().saturating_sub(1);
        self.character_cursor = self.character_cursor.min(last);
    }

    /// Move the character-list cursor. Saturates at both ends instead of
    /// wrapping — a list you can overshoot off the bottom and reappear at the
    /// top of is easy to activate the wrong entry from.
    pub fn move_character_cursor(&mut self, delta: isize) {
        if self.characters.is_empty() {
            self.character_cursor = 0;
            return;
        }
        let last = self.characters.len() - 1;
        let next = (self.character_cursor as isize).saturating_add(delta);
        self.character_cursor = next.clamp(0, last as isize) as usize;
    }

    /// The character the cursor is on, if the list is non-empty.
    pub fn selected_character(&self) -> Option<&CharacterEntry> {
        self.characters.get(self.character_cursor)
    }

    pub fn module_states(&self) -> [(&'static str, bool); 7] {
        [
            ("talk", self.modules.talk),
            ("memory", self.modules.memory),
            ("speech", self.modules.speech),
            ("vision", self.modules.vision),
            ("action", self.modules.action),
            ("scheduler", self.modules.scheduler),
            ("translation", self.modules.translation),
        ]
    }

    /// Take the current input box contents for sending, clearing the box.
    /// Returns `None` for blank/whitespace-only input so callers don't have
    /// to special-case "nothing to send" themselves — mirrors the web UI's
    /// composer, which also refuses to send an empty message.
    pub fn take_input_for_send(&mut self) -> Option<String> {
        let text = self.input.trim().to_string();
        self.input.clear();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modules_all_off() -> ModuleFlags {
        ModuleFlags::default()
    }

    #[test]
    fn default_app_starts_connecting_focused_on_input() {
        let app = App::default();
        assert_eq!(app.connection, ConnectionState::Connecting);
        assert_eq!(app.view, ViewState::Main);
        assert_eq!(app.focus, Focus::Main(MainFocus::Input));
        assert!(app.chat.is_empty());
        assert!(!app.pending);
    }

    #[test]
    fn hello_sets_version_modules_and_character() {
        let mut app = App::default();
        app.apply_server_msg(ServerMsg::Hello {
            version: "1".to_string(),
            modules: ModuleFlags {
                talk: true,
                ..modules_all_off()
            },
            character: Some(CharacterRef {
                id: "c1".to_string(),
                name: "Rin".to_string(),
            }),
        });
        assert_eq!(app.version.as_deref(), Some("1"));
        assert!(app.modules.talk);
        assert_eq!(app.character.unwrap().name, "Rin");
    }

    #[test]
    fn assistant_chat_clears_pending_but_user_chat_does_not() {
        let mut app = App::default();
        app.pending = true;
        app.apply_server_msg(ServerMsg::Chat {
            role: "user".to_string(),
            text: "hi".to_string(),
            ts: 0,
        });
        assert!(
            app.pending,
            "a user echo shouldn't clear the typing indicator"
        );

        app.apply_server_msg(ServerMsg::Chat {
            role: "assistant".to_string(),
            text: "yo".to_string(),
            ts: 0,
        });
        assert!(!app.pending);
        assert_eq!(app.chat.len(), 2);
    }

    #[test]
    fn silent_frame_clears_pending_and_records_reason() {
        let mut app = App::default();
        app.pending = true;
        app.apply_server_msg(ServerMsg::Silent {
            reason: "declined".to_string(),
            ts: 0,
        });
        assert!(!app.pending);
        match &app.chat[0] {
            ChatRow::Silent { reason } => assert_eq!(reason, "declined"),
            other => panic!("expected Silent, got {other:?}"),
        }
    }

    #[test]
    fn input_accepted_sets_pending() {
        let mut app = App::default();
        assert!(!app.pending);
        app.apply_server_msg(ServerMsg::InputAccepted {
            request_id: "r1".to_string(),
        });
        assert!(app.pending);
    }

    #[test]
    fn error_frame_sets_status_line_and_clears_pending() {
        let mut app = App::default();
        app.pending = true;
        app.apply_server_msg(ServerMsg::Error {
            message: "boom".to_string(),
        });
        assert!(!app.pending);
        assert_eq!(app.status_line.as_deref(), Some("error: boom"));
    }

    #[test]
    fn response_error_status_sets_status_line() {
        let mut app = App::default();
        app.apply_server_msg(ServerMsg::Response {
            request_id: "r1".to_string(),
            status: "error".to_string(),
            text: None,
            message: Some("nope".to_string()),
        });
        assert_eq!(app.status_line.as_deref(), Some("error: nope"));
    }

    #[test]
    fn unknown_frame_goes_to_log_not_chat() {
        // A one-off frame kind, deliberately not one of the continuous ones
        // `is_high_frequency` filters out — this test is about *where* an
        // unmodeled frame lands, and picking `speakingLevel` here would
        // silently retest the filter instead.
        let mut app = App::default();
        app.apply_server_msg(ServerMsg::Unknown("translation".to_string()));
        assert!(app.chat.is_empty());
        assert_eq!(app.log.len(), 1);
        assert_eq!(app.log[0].label, "unhandled");
        assert_eq!(app.log[0].text, "translation");
    }

    #[test]
    fn chat_rows_are_capped_at_max_rows() {
        let mut app = App::default();
        for i in 0..(MAX_ROWS + 10) {
            app.apply_server_msg(ServerMsg::Chat {
                role: "user".to_string(),
                text: i.to_string(),
                ts: 0,
            });
        }
        assert_eq!(app.chat.len(), MAX_ROWS);
        // The oldest 10 rows should have been dropped, so row 0 is gone and
        // the surviving oldest row is index 10's original text.
        match &app.chat[0] {
            ChatRow::Chat { text, .. } => assert_eq!(text, "10"),
            other => panic!("expected Chat, got {other:?}"),
        }
    }

    #[test]
    fn main_focus_cycles_input_chat_log_and_wraps() {
        let mut app = App::default();
        assert_eq!(app.focus, Focus::Main(MainFocus::Input));
        app.next_focus();
        assert_eq!(app.focus, Focus::Main(MainFocus::Chat));
        app.next_focus();
        assert_eq!(app.focus, Focus::Main(MainFocus::Log));
        app.next_focus();
        assert_eq!(
            app.focus,
            Focus::Main(MainFocus::Input),
            "cycling forward past the last Main target should wrap to the first"
        );

        app.prev_focus();
        assert_eq!(
            app.focus,
            Focus::Main(MainFocus::Log),
            "cycling backward past the first Main target should wrap to the last"
        );
    }

    #[test]
    /// Continuous telemetry must not reach the log panel. Observed on a real
    /// server before this filter existed: the panel was a solid wall of
    /// `[unhandled] volume` with nothing else visible.
    #[test]
    fn continuous_telemetry_frames_are_dropped_not_logged() {
        let mut app = App::default();
        for _ in 0..50 {
            app.apply_server_msg(ServerMsg::Unknown("volume".into()));
            app.apply_server_msg(ServerMsg::Unknown("speakingLevel".into()));
        }
        assert!(
            app.log.is_empty(),
            "high-frequency frames should never reach the log: {:?}",
            app.log
        );

        // A one-off unmodeled frame still gets its trace line — the point is
        // to filter the firehose, not to go silent.
        app.apply_server_msg(ServerMsg::Unknown("vrmPose".into()));
        assert_eq!(app.log.len(), 1);
        assert_eq!(app.log[0].text, "vrmPose");
    }

    #[test]
    fn config_focus_cycles_within_itself_only() {
        let mut app = App::default();
        app.toggle_view();
        assert_eq!(app.focus, Focus::Config(ConfigFocus::Modules));
        app.next_focus();
        assert_eq!(
            app.focus,
            Focus::Config(ConfigFocus::Characters),
            "Tab should move to the other Config panel"
        );
        app.next_focus();
        assert_eq!(
            app.focus,
            Focus::Config(ConfigFocus::Modules),
            "focus must wrap inside the Config screen, never spill into Main's"
        );
        app.prev_focus();
        assert_eq!(app.focus, Focus::Config(ConfigFocus::Characters));
    }

    #[test]
    fn toggle_view_switches_screen_and_resets_focus() {
        let mut app = App::default();
        app.focus = Focus::Main(MainFocus::Log);

        app.toggle_view();
        assert_eq!(app.view, ViewState::Config);
        assert_eq!(
            app.focus,
            Focus::Config(ConfigFocus::Modules),
            "entering Config should reset focus, not carry over Main's"
        );

        app.toggle_view();
        assert_eq!(app.view, ViewState::Main);
        assert_eq!(app.focus, Focus::Main(MainFocus::Input));
    }

    #[test]
    fn esc_returns_to_main_from_config_but_is_a_no_op_on_main() {
        let mut app = App::default();
        app.return_to_main();
        assert_eq!(
            app.view,
            ViewState::Main,
            "Esc on the Main screen should not do anything surprising"
        );

        app.toggle_view();
        assert_eq!(app.view, ViewState::Config);
        app.return_to_main();
        assert_eq!(app.view, ViewState::Main);
        assert_eq!(app.focus, Focus::Main(MainFocus::Input));
    }

    #[test]
    fn event_log_is_capped_and_drops_oldest_first() {
        let mut app = App::default();
        for i in 0..(EVENT_LOG_LIMIT + 5) {
            app.add_event_log(i.to_string());
        }
        assert_eq!(app.event_log.len(), EVENT_LOG_LIMIT);
        assert_eq!(
            app.event_log[0], "5",
            "the oldest 5 rows should have been dropped"
        );
    }

    #[test]
    fn event_log_ignores_blank_messages() {
        let mut app = App::default();
        app.add_event_log("   ");
        assert!(app.event_log.is_empty());
    }

    #[test]
    fn set_connection_logs_the_transition_as_an_event() {
        let mut app = App::default();
        app.set_connection(ConnectionState::Connected);
        assert_eq!(app.event_log.len(), 1);
        assert!(app.event_log[0].contains(ConnectionState::Connected.label()));

        // Re-asserting the same state is not a transition and shouldn't
        // add a second entry.
        app.set_connection(ConnectionState::Connected);
        assert_eq!(app.event_log.len(), 1);
    }

    #[test]
    fn error_expires_after_ttl_via_tick_but_not_before() {
        let mut app = App::default();
        let t0 = Instant::now();
        app.set_error("boom", t0);
        assert!(app.error.is_some());

        // Still within the TTL window: tick must not clear it.
        app.tick(t0 + Duration::from_secs(5));
        assert!(
            app.error.is_some(),
            "error should still be visible before ERROR_TTL elapses"
        );

        // Past the TTL window: tick clears it.
        app.tick(t0 + Duration::from_secs(11));
        assert!(
            app.error.is_none(),
            "error should be cleared once ERROR_TTL has elapsed"
        );
    }

    #[test]
    fn notice_has_no_ttl_and_persists_until_replaced_or_cleared() {
        let mut app = App::default();
        app.set_notice("しきい値を更新");
        app.tick(Instant::now() + Duration::from_secs(3600));
        assert_eq!(
            app.notice.as_deref(),
            Some("しきい値を更新"),
            "notice has no TTL, so tick must never clear it on its own"
        );

        app.clear_notice();
        assert_eq!(app.notice, None);
    }

    #[test]
    fn status_frame_updates_module_states() {
        let mut app = App::default();
        assert!(!app.modules.talk);
        app.apply_server_msg(ServerMsg::Status {
            modules: ModuleFlags {
                talk: true,
                speech: true,
                ..modules_all_off()
            },
        });
        assert!(app.modules.talk);
        assert!(app.modules.speech);
        assert!(!app.modules.memory);
        assert_eq!(
            app.module_states(),
            [
                ("talk", true),
                ("memory", false),
                ("speech", true),
                ("vision", false),
                ("action", false),
                ("scheduler", false),
                ("translation", false),
            ]
        );
    }

    #[test]
    fn system_activity_reflects_connection_and_pending_state() {
        let mut app = App::default();
        app.set_connection(ConnectionState::Disconnected);
        assert_eq!(app.system_activity(), SystemActivity::Offline);

        app.set_connection(ConnectionState::Reconnecting);
        assert_eq!(app.system_activity(), SystemActivity::Connecting);

        app.set_connection(ConnectionState::Connected);
        assert_eq!(app.system_activity(), SystemActivity::Idle);

        app.pending = true;
        assert_eq!(app.system_activity(), SystemActivity::Waiting);
    }

    #[test]
    fn take_input_for_send_clears_box_and_trims() {
        let mut app = App::default();
        app.input = "  こんにちは  ".to_string();
        let sent = app.take_input_for_send();
        assert_eq!(sent.as_deref(), Some("こんにちは"));
        assert_eq!(app.input, "");
    }

    #[test]
    fn take_input_for_send_returns_none_for_blank_input() {
        let mut app = App::default();
        app.input = "   ".to_string();
        assert_eq!(app.take_input_for_send(), None);
        assert_eq!(app.input, "");
    }

    #[test]
    fn set_connection_updates_state() {
        let mut app = App::default();
        app.set_connection(ConnectionState::Connected);
        assert_eq!(app.connection, ConnectionState::Connected);
    }
}
