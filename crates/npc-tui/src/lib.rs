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

mod app;
mod protocol;
mod ui;
mod ws_client;

pub use app::{App, ConnectionState, Focus};
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

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    addr: SocketAddr,
) -> anyhow::Result<()> {
    let mut app = App::default();
    let (mut client_events, outgoing, ws_handle) = ws_client::spawn(addr);
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
            term_event = term_events.next() => {
                match term_event {
                    Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                        handle_key(&mut app, &outgoing, key.code, key.modifiers);
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => break,
                }
            }
        }

        terminal.draw(|frame| ui::draw(frame, &app))?;
    }

    ws_handle.abort();
    Ok(())
}

/// Page-scroll step, in lines, for PageUp/PageDown — an arbitrary but
/// generous jump (most terminal panels are well under this many rows tall)
/// so one keypress reliably moves by "about a screenful" without this
/// crate needing to know the panel's actual current height.
const PAGE_SCROLL_LINES: u16 = 20;

fn handle_key(
    app: &mut App,
    outgoing: &tokio::sync::mpsc::UnboundedSender<protocol::ClientMsg>,
    code: KeyCode,
    modifiers: KeyModifiers,
) {
    if code == KeyCode::Esc
        || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL))
    {
        app.should_quit = true;
        return;
    }

    if code == KeyCode::Tab {
        app.toggle_focus();
        return;
    }

    // Scroll keys act on whichever panel is focused (see `Focus`'s doc for
    // why that's a clean 1:1 mapping in this layout: Input <-> chat, Log <->
    // log) regardless of what else is bound below.
    let scroll = if app.focus == Focus::Input {
        &mut app.chat_scroll
    } else {
        &mut app.log_scroll
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

    if app.focus != Focus::Input {
        // Log-focused: no text entry to accept, and 'q' is a convenience
        // quit key here specifically because it can't collide with typing
        // (there is nothing to type into while the log panel has focus).
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
