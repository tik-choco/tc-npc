//! Owns the WebSocket connection to npc-server's `/ws` endpoint and bridges
//! it to the rest of the app as two plain channels: an [`mpsc::UnboundedReceiver<ClientEvent>`]
//! carrying connection-state transitions and parsed [`ServerMsg`] frames, and
//! an [`mpsc::UnboundedSender<ClientMsg>`] the UI loop uses to queue outgoing
//! frames.
//!
//! Reconnects on drop with the same backoff shape as `web/src/lib/ws.ts`'s
//! `NpcSocket` (1s, doubling to a 5s cap) — a restarted server (`just
//! watch`, or an operator bouncing `tc-npc serve` on the far end of an SSH
//! session) should heal on its own rather than requiring the TUI to be
//! relaunched. Unlike the browser client this has no window-focus/online
//! event to retry early on — a terminal has no such signal — so it just
//! keeps to the backoff schedule unconditionally.

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::app::ConnectionState;
use crate::protocol::{ClientMsg, ServerMsg};

const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(5);

/// One event flowing from the connection task back to the UI loop.
pub enum ClientEvent {
    Connection(ConnectionState),
    Server(ServerMsg),
}

/// Spawn the connection-owning task and return the channel pair plus its
/// `JoinHandle`. The caller (`lib::run`) aborts the handle on quit rather
/// than waiting for a graceful shutdown handshake — same call npc-server's
/// own `run_server` makes about its stale `server-port.txt` (see that
/// module's doc comment): a slightly abrupt teardown of a connection that's
/// closing anyway is a smaller problem than a more elaborate shutdown path
/// built solely to avoid it.
pub fn spawn(
    addr: SocketAddr,
) -> (
    mpsc::UnboundedReceiver<ClientEvent>,
    mpsc::UnboundedSender<ClientMsg>,
    tokio::task::JoinHandle<()>,
) {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (outgoing_tx, outgoing_rx) = mpsc::unbounded_channel();
    let handle = tokio::spawn(reconnect_loop(addr, event_tx, outgoing_rx));
    (event_rx, outgoing_tx, handle)
}

async fn reconnect_loop(
    addr: SocketAddr,
    events: mpsc::UnboundedSender<ClientEvent>,
    mut outgoing: mpsc::UnboundedReceiver<ClientMsg>,
) {
    // Plain `ws://`, never `wss://` — see Cargo.toml's comment on why
    // tokio-tungstenite is pulled in without any TLS backend at all.
    let url = format!("ws://{addr}/ws");
    let mut backoff = RECONNECT_MIN;
    let mut ever_connected = false;

    loop {
        let connecting_state = if ever_connected {
            ConnectionState::Reconnecting
        } else {
            ConnectionState::Connecting
        };
        if events
            .send(ClientEvent::Connection(connecting_state))
            .is_err()
        {
            return; // UI loop is gone; nothing left to report to.
        }

        match tokio_tungstenite::connect_async(&url).await {
            Ok((ws_stream, _response)) => {
                backoff = RECONNECT_MIN;
                ever_connected = true;
                if events
                    .send(ClientEvent::Connection(ConnectionState::Connected))
                    .is_err()
                {
                    return;
                }
                if run_connection(ws_stream, &events, &mut outgoing)
                    .await
                    .is_err()
                {
                    // Outgoing channel closed = the UI is quitting, not a
                    // connection problem: stop reconnecting instead of
                    // spinning against a server that's still perfectly fine.
                    return;
                }
                if events
                    .send(ClientEvent::Connection(ConnectionState::Disconnected))
                    .is_err()
                {
                    return;
                }
            }
            Err(err) => {
                // Surfaced as a `ServerMsg::Error` (not a separate event
                // kind) so a failed connect attempt shows up exactly where
                // any other error would in the log panel / status line,
                // without `App` needing to know about connection-attempt
                // failures as their own concept.
                if events
                    .send(ClientEvent::Server(ServerMsg::Error {
                        message: format!("connect failed: {err}"),
                    }))
                    .is_err()
                {
                    return;
                }
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

/// Pump one live connection until it closes, forwarding text frames in as
/// parsed [`ServerMsg`]s and outgoing [`ClientMsg`]s out. Returns `Err(())`
/// only when `outgoing`'s sender has been dropped (the UI is quitting) so
/// the caller can tell that apart from an ordinary connection drop, which
/// should trigger a reconnect rather than shutting the whole task down.
async fn run_connection<S>(
    ws_stream: tokio_tungstenite::WebSocketStream<S>,
    events: &mpsc::UnboundedSender<ClientEvent>,
    outgoing: &mut mpsc::UnboundedReceiver<ClientMsg>,
) -> Result<(), ()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws_stream.split();
    loop {
        tokio::select! {
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let msg = match ServerMsg::parse(&text) {
                            Ok(msg) => msg,
                            Err(err) => ServerMsg::Error { message: format!("malformed frame: {err}") },
                        };
                        if events.send(ClientEvent::Server(msg)).is_err() {
                            return Err(());
                        }
                    }
                    // Ping/Pong/Binary/Frame: nothing this client does with
                    // any of them — tungstenite answers Ping with Pong
                    // itself under the hood, so there's no reply to send.
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => return Ok(()),
                }
            }
            outgoing_msg = outgoing.recv() => {
                match outgoing_msg {
                    Some(msg) => {
                        // A serialize failure here would mean `ClientMsg`
                        // itself is unrepresentable as JSON, which can't
                        // happen for the plain-string fields it has — but
                        // dropping the frame rather than panicking keeps a
                        // freak failure from taking the whole TUI down.
                        if let Ok(text) = serde_json::to_string(&msg) {
                            if sink.send(Message::Text(text)).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                    None => return Err(()),
                }
            }
        }
    }
}
