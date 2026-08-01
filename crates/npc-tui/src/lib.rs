//! Terminal UI client for tc-npc.
//!
//! This crate is a **client of `npc-server`'s HTTP/WebSocket API**, not a
//! second front-end wired into the module bus. It speaks exactly the same
//! `/ws` protocol as `web/src/` (see `crates/npc-server/src/protocol.rs` and
//! this crate's `protocol.rs` for the subset actually consumed here), which
//! is what lets it attach to a `tc-npc` process over an SSH port-forward
//! just as well as to one running on the same machine — there is no direct
//! dependency on any `npc-*` crate besides this one's own wire types. See
//! the module docs on `protocol`, `app`, `ws_client`, and `ui` for how the
//! pieces divide up: parsing/serializing the wire format, pure state
//! transitions, the WebSocket connection (with reconnect), and rendering,
//! respectively — kept in separate files specifically so the state-
//! transition logic in `app.rs` can be unit tested without a real terminal
//! or socket.
//!
//! v1 scope (see the worker brief that commissioned this crate): chat send/
//! receive, a status line (connection/modules/active character), a log
//! panel for sense/memory/action-log/unmodeled frames, and quit + focus
//! switching. Deliberately not attempting to port any of the web UI's other
//! nine tabs (VRM avatar, 感情 sparkline, 通訳, 人物, etc.) — see
//! `protocol.rs`'s module doc for why frames belonging to those simply land
//! in the log panel labeled `unhandled` instead.

mod api;
mod app;
mod protocol;
mod theme;
mod ui;
mod widgets;
mod ws_client;

pub use app::{App, ConfigFocus, ConnectionState, Focus, MainFocus, SystemActivity, ViewState};
pub use protocol::{ClientMsg, ServerMsg};

use std::io;
use std::net::SocketAddr;

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use ws_client::ClientEvent;

/// npc-server's compiled-in default `server.addr` (see
/// `npc_core::config::default_server_addr` /
/// `crates/npc-core/src/config.rs`'s `"127.0.0.1:47950"`). Duplicated here
/// rather than depended on: pulling in all of `npc-core` (module bus,
/// config schema, character/VRM/person storage — the entire non-server
/// surface of tc-npc) just for one string literal would undercut the
/// point of this crate being a thin, decoupled HTTP/WS client. If the
/// default ever moves, `resolve_addr`'s `server-port.txt` path is the one
/// that matters for an already-running server anyway; this constant is only
/// ever the last-resort guess for "nothing is running yet, or its port file
/// vanished".
const DEFAULT_ADDR: &str = "127.0.0.1:47950";

/// File name written by npc-server under `~/.tc-npc/` publishing whichever
/// port it actually bound (see `npc_server::SERVER_PORT_FILE_NAME`) — kept
/// in sync with that constant by hand for the same reason `DEFAULT_ADDR`
/// above is: this crate deliberately doesn't depend on `npc-server` either.
const SERVER_PORT_FILE_NAME: &str = "server-port.txt";

/// Resolve which address to connect to: `explicit`, if given (e.g. a future
/// `tc-npc tui --addr` flag), always wins. Otherwise, read the port
/// npc-server last actually bound from `~/.tc-npc/server-port.txt` — plain
/// decimal text, nothing else (see that constant's doc) — since the
/// configured port in `config.json` isn't authoritative: `bind_with_retry`
/// falls forward to the next port whenever the configured one is taken, and
/// the port file is the only thing an out-of-process client has to learn
/// which one actually won. Falling back to [`DEFAULT_ADDR`] when the file is
/// missing/unreadable/non-numeric is a best-effort guess for "the server
/// hasn't started yet, or was never asked to write it" — not a claim that
/// something is listening there.
pub fn resolve_addr(explicit: Option<SocketAddr>) -> anyhow::Result<SocketAddr> {
    if let Some(addr) = explicit {
        return Ok(addr);
    }

    let port = dirs::home_dir()
        .map(|home| home.join(".tc-npc").join(SERVER_PORT_FILE_NAME))
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|contents| contents.trim().parse::<u16>().ok());

    let addr = match port {
        Some(port) => format!("127.0.0.1:{port}"),
        None => DEFAULT_ADDR.to_string(),
    };
    addr.parse()
        .map_err(|e| anyhow::anyhow!("npc-tui: could not parse resolved address {addr:?}: {e}"))
}

/// Run the TUI against the server at `addr` until the user quits (`q`/Esc/
/// Ctrl+C). Takes over the terminal (raw mode + alternate screen) for the
/// duration and always restores it on the way out, including on error —
/// see the `defer`-style guard built with a closure below, since Rust has no
/// built-in `finally`.
pub async fn run(addr: SocketAddr) -> anyhow::Result<()> {
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, addr).await;

    // Best-effort terminal restoration regardless of how `run_app` exited:
    // leaving the terminal in raw/alternate-screen mode after a panic or
    // error would strand the user's shell in a broken-looking state, which
    // is a worse failure than this teardown itself failing silently.
    let _ = disable_raw_mode();
    let _ = io::stdout().execute(LeaveAlternateScreen);

    result
}

/// Results of the REST calls the Config screen makes. They arrive on their
/// own channel rather than being awaited inline: `GET /api/characters` is
/// allowed up to `api`'s 5-second timeout, and awaiting that in the event
/// loop would freeze the whole UI — including the quit key — whenever the
/// server is wedged. That is the failure this indirection exists to prevent.
enum ApiEvent {
    Characters(Vec<app::CharacterEntry>),
    Failed(String),
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    addr: SocketAddr,
) -> anyhow::Result<()> {
    let mut app = App::default();
    let (mut client_events, outgoing, ws_handle) = ws_client::spawn(addr);
    let (api_tx, mut api_events) = tokio::sync::mpsc::unbounded_channel::<ApiEvent>();
    let mut term_events = EventStream::new();

    terminal.draw(|frame| ui::draw(frame, &app))?;

    while !app.should_quit {
        tokio::select! {
            event = client_events.recv() => {
                match event {
                    Some(ClientEvent::Connection(state)) => app.set_connection(state),
                    Some(ClientEvent::Server(msg)) => app.apply_server_msg(msg),
                    // The ws_client task only ever exits by returning from
                    // `reconnect_loop`, which itself only happens when the
                    // outgoing sender (owned by this same function, below)
                    // is dropped -- i.e. after this loop has already ended.
                    // A `None` here while the loop is still running would
                    // mean that task panicked; nothing productive to do but
                    // stop, since there is no connection left to drive.
                    None => break,
                }
            }
            api_event = api_events.recv() => {
                match api_event {
                    Some(ApiEvent::Characters(list)) => app.set_characters(list),
                    Some(ApiEvent::Failed(message)) => {
                        app.set_error(message, std::time::Instant::now());
                    }
                    // The sender is held by this function, so it outlives the
                    // loop; `None` cannot happen while we're still running.
                    None => {}
                }
            }
            term_event = term_events.next() => {
                match term_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        let before = app.view;
                        handle_key(&mut app, &outgoing, addr, &api_tx, key.code, key.modifiers);
                        // Refetch on the way *into* the Config screen, and
                        // after activating a character, so the list is never
                        // stale from a previous visit.
                        if app.view == ViewState::Config
                            && (before != ViewState::Config || app.characters.is_empty())
                        {
                            spawn_character_fetch(addr, api_tx.clone());
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
        }

        // `App` never reads the clock itself — that is what keeps its state
        // transitions testable without faking time (see `app.rs`). The cost
        // is that something has to hand it "now" so timed things can expire;
        // once per frame, here, is that something.
        app.tick(std::time::Instant::now());
        terminal.draw(|frame| ui::draw(frame, &app))?;
    }

    ws_handle.abort();
    Ok(())
}

/// Fetch the character list off the event loop and post the result back.
/// Detached on purpose: see [`ApiEvent`].
fn spawn_character_fetch(addr: SocketAddr, tx: tokio::sync::mpsc::UnboundedSender<ApiEvent>) {
    tokio::spawn(async move {
        let event = match api::list_characters(addr).await {
            Ok(list) => ApiEvent::Characters(
                list.into_iter()
                    .map(|c| app::CharacterEntry {
                        id: c.id,
                        name: c.name,
                        active: c.active,
                    })
                    .collect(),
            ),
            Err(e) => ApiEvent::Failed(format!("キャラクタ一覧の取得に失敗: {e}")),
        };
        // A send failure means the UI already exited; nothing to report to.
        let _ = tx.send(event);
    });
}

/// Activate a character, then refetch so the `active` flags on screen match
/// what the server now thinks. Same detached shape, same reason.
fn spawn_character_activate(
    addr: SocketAddr,
    id: String,
    tx: tokio::sync::mpsc::UnboundedSender<ApiEvent>,
) {
    tokio::spawn(async move {
        if let Err(e) = api::activate_character(addr, &id).await {
            let _ = tx.send(ApiEvent::Failed(format!("キャラクタの切替に失敗: {e}")));
            return;
        }
        match api::list_characters(addr).await {
            Ok(list) => {
                let _ = tx.send(ApiEvent::Characters(
                    list.into_iter()
                        .map(|c| app::CharacterEntry {
                            id: c.id,
                            name: c.name,
                            active: c.active,
                        })
                        .collect(),
                ));
            }
            Err(e) => {
                let _ = tx.send(ApiEvent::Failed(format!("一覧の再取得に失敗: {e}")));
            }
        }
    });
}

/// Page-scroll step, in lines, for PageUp/PageDown — an arbitrary but
/// generous jump (most terminal panels are well under this many rows tall)
/// so one keypress reliably moves by "about a screenful" without this
/// crate needing to know the panel's actual current height.
const PAGE_SCROLL_LINES: u16 = 20;

fn handle_key(
    app: &mut App,
    outgoing: &tokio::sync::mpsc::UnboundedSender<protocol::ClientMsg>,
    // The Config screen's one write action (activating a character) needs to
    // reach the server, and it must not do so inline — see [`ApiEvent`].
    addr: SocketAddr,
    api_tx: &tokio::sync::mpsc::UnboundedSender<ApiEvent>,
    code: KeyCode,
    modifiers: KeyModifiers,
) {
    // Ctrl+C is the only unconditional quit. `q` cannot be, because the main
    // view has a text box and typing "q" must produce a q; `Esc` cannot be
    // either, now that there are two screens and Esc's job is backing out of
    // the inner one.
    if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
        return;
    }

    // Esc backs out one level: Config -> Main, and Main -> quit. `q` from a
    // non-text focus is the same idea by a shorter route (handled below,
    // where it is known not to collide with typing).
    if code == KeyCode::Esc {
        if app.view == ViewState::Config {
            app.return_to_main();
        } else {
            app.should_quit = true;
        }
        return;
    }

    // F2 rather than Tab for switching screens: Tab is spent on cycling
    // focus *within* a screen, which this layout needs because there are
    // three focusable panels on the main view. A function key also can't be
    // swallowed by the input box.
    if code == KeyCode::F(2) {
        app.toggle_view();
        return;
    }

    if code == KeyCode::Tab {
        if modifiers.contains(KeyModifiers::SHIFT) {
            app.prev_focus();
        } else {
            app.next_focus();
        }
        return;
    }
    if code == KeyCode::BackTab {
        app.prev_focus();
        return;
    }

    if app.view == ViewState::Config {
        // The module list is read-only: those flags are read once at startup
        // to decide what to spawn, so a toggle here would silently do nothing
        // until relaunch (see `ui::RESTART_REQUIRED`). Character switching is
        // the opposite — the server applies it immediately — so that is what
        // this screen actually lets you do.
        if app.focus == Focus::Config(ConfigFocus::Characters) {
            match code {
                KeyCode::Up => app.move_character_cursor(-1),
                KeyCode::Down => app.move_character_cursor(1),
                KeyCode::Enter => {
                    if let Some(entry) = app.selected_character() {
                        let (id, name) = (entry.id.clone(), entry.name.clone());
                        app.set_notice(format!("{name} に切り替えています..."));
                        app.add_event_log(format!("activate character: {name}"));
                        spawn_character_activate(addr, id, api_tx.clone());
                    }
                }
                _ => {}
            }
        }
        return;
    }

    // Scroll keys act on whichever panel is focused. Input focus scrolls the
    // transcript, since that is what you are reading while you type.
    let scroll = match app.focus {
        Focus::Main(MainFocus::Log) => &mut app.log_scroll,
        _ => &mut app.chat_scroll,
    };
    match code {
        KeyCode::Up => {
            *scroll = scroll.saturating_add(1);
            return;
        }
        KeyCode::Down => {
            *scroll = scroll.saturating_sub(1);
            return;
        }
        KeyCode::PageUp => {
            *scroll = scroll.saturating_add(PAGE_SCROLL_LINES);
            return;
        }
        KeyCode::PageDown => {
            *scroll = scroll.saturating_sub(PAGE_SCROLL_LINES);
            return;
        }
        _ => {}
    }

    if app.focus != Focus::Main(MainFocus::Input) {
        // Not the text box: `q` is a convenience quit here specifically
        // because it cannot collide with typing.
        if code == KeyCode::Char('q') {
            app.should_quit = true;
        }
        return;
    }

    match code {
        KeyCode::Enter => {
            if let Some(text) = app.take_input_for_send() {
                // `speaker: None` — see `protocol::ClientMsg`'s doc: this
                // crate has no login/identity concept to attribute a
                // speaker name with, unlike the extension API's `event`
                // frames.
                let _ = outgoing.send(protocol::ClientMsg::Input {
                    text,
                    speaker: None,
                });
            }
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Char(c) => {
            app.input.push(c);
        }
        _ => {}
    }
}
