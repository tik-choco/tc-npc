//! In-memory TUI state and the pure state-transition logic driving it.
//! Deliberately free of any ratatui/crossterm/tokio types: everything here
//! is a plain data transformation from an incoming [`AppEvent`] to the next
//! `App`, so [`App::handle_event`]'s branches can be exercised by a unit
//! test without standing up a terminal or a socket (see `ui.rs` for the only
//! part of this crate that actually touches the screen).

use crate::protocol::{CharacterRef, ModuleFlags, ServerMsg};

/// How many rows [`App::chat`]/[`App::log`] keep before dropping the oldest.
/// A TUI session can run for a long time (SSH'd into a machine and left
/// open); this bounds memory the same way `web/src/hooks/useNpcSocket.ts`'s
/// `MAX_ENTRIES` bounds the browser's, without needing to match its exact
/// number — there is no shared contract on scrollback length between the two
/// clients, only the general principle that neither should grow forever.
const MAX_ROWS: usize = 500;

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

/// Which panel has keyboard focus. Only the input box and the log panel are
/// meaningfully "focusable" in v1: the chat transcript scrolls with the same
/// keys regardless (see `ui.rs`'s key handling), so there's no separate
/// `Chat` focus state to switch to — narrowing this to two members here
/// keeps `Tab` a plain toggle instead of needing to cycle a longer list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Log,
}

impl Focus {
    pub fn toggled(self) -> Focus {
        match self {
            Focus::Input => Focus::Log,
            Focus::Log => Focus::Input,
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
    pub status_line: Option<String>,
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
            focus: Focus::Input,
            chat_scroll: 0,
            log_scroll: 0,
            pending: false,
            should_quit: false,
            status_line: None,
        }
    }
}

fn push_capped<T>(rows: &mut Vec<T>, row: T) {
    rows.push(row);
    if rows.len() > MAX_ROWS {
        // Drop from the front: `rows` is arrival-ordered oldest-first, same
        // convention as the web UI's `cap()` helper.
        let excess = rows.len() - MAX_ROWS;
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
                push_capped(&mut self.chat, ChatRow::Chat { role, text });
                if is_assistant {
                    self.pending = false;
                }
            }
            ServerMsg::Silent { reason, .. } => {
                push_capped(&mut self.chat, ChatRow::Silent { reason });
                self.pending = false;
            }
            ServerMsg::Sense { kind, text, .. } => {
                push_capped(
                    &mut self.log,
                    LogRow {
                        label: format!("sense:{kind}"),
                        text,
                    },
                );
            }
            ServerMsg::Memory { kind, text } => {
                push_capped(
                    &mut self.log,
                    LogRow {
                        label: format!("memory:{kind}"),
                        text,
                    },
                );
            }
            ServerMsg::ActionLog { text } => {
                push_capped(
                    &mut self.log,
                    LogRow {
                        label: "action".to_string(),
                        text,
                    },
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
                    );
                }
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
                );
            }
        }
    }

    pub fn set_connection(&mut self, state: ConnectionState) {
        self.connection = state;
    }

    pub fn toggle_focus(&mut self) {
        self.focus = self.focus.toggled();
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
        assert_eq!(app.focus, Focus::Input);
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
        let mut app = App::default();
        app.apply_server_msg(ServerMsg::Unknown("speakingLevel".to_string()));
        assert!(app.chat.is_empty());
        assert_eq!(app.log.len(), 1);
        assert_eq!(app.log[0].label, "unhandled");
        assert_eq!(app.log[0].text, "speakingLevel");
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
    fn toggle_focus_flips_between_input_and_log() {
        let mut app = App::default();
        assert_eq!(app.focus, Focus::Input);
        app.toggle_focus();
        assert_eq!(app.focus, Focus::Log);
        app.toggle_focus();
        assert_eq!(app.focus, Focus::Input);
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
