//! A thin REST client for npc-server's `/api/*` endpoints, backing the
//! Config screen. Where `ws_client.rs` owns one long-lived `/ws` connection
//! for the whole session, this module opens a fresh, short-lived TCP
//! connection per call 窶・REST here is stateless request/response, and
//! npc-server is a single local process on `127.0.0.1`, so there is no
//! connection pool worth building.
//!
//! ## What the Config screen may and may not offer
//!
//! Only two calls are implemented, and the omissions are deliberate. Reading
//! and writing the whole config (`GET`/`PUT /api/config`) was mapped out and
//! then left unimplemented, because a settings *editor* cannot be built
//! honestly until the caller is ready to respect two properties of that
//! endpoint:
//!
//! **`PUT /api/config` takes the whole document, not a patch.** Every field
//! of npc-server's `Config` is `#[serde(default)]`, so a document missing a
//! section deserializes that section as its *default* on the server 窶・
//! silently resetting it, not "leave whatever was there". The only correct
//! pattern is GET, mutate the returned value in place, PUT the same value
//! back; never assemble one from scratch. (A saved `"api_key": "***"`
//! placeholder is safe to round-trip: the server restores the real secret
//! behind it, matched positionally and, for `providers[]`, by `id`.)
//!
//! **Only some settings apply without a restart.** A successful PUT
//! republishes the document on the `npc:config` bus topic, but only some
//! running modules subscribe:
//!
//! | Immediate | Needs a process restart |
//! |---|---|
//! | `scheduler.*` (announcements, schedule on/off) | `talk` / `memory` / `vision` / `action` / `mist` 窶・only *spawned* at startup when their flag is on |
//! | `action.locations`, `action.routes` | `api.*` 窶・the LLM connection npc-talk/npc-memory/npc-vision hold is read once at startup |
//! | `speech.*` 窶・stt/tts on/off, devices, endpoint, model, VAD knobs | |
//! | `translation.*` 窶・mode, languages | |
//! | `character.active_id` 窶・via [`activate_character`], which republishes `npc:config` itself | |
//!
//! That right-hand column is why the Config screen shows the module flags
//! read-only and offers character switching as its one write action: a
//! checkbox that appears to work and does nothing until the next launch is
//! worse than no checkbox. Implement the config editor when the screen can
//! flag those fields per-row; the mapping above is the hard part and is
//! recorded here so it doesn't have to be rediscovered.
//!
//! ## Why this hand-rolls HTTP/1.1 instead of using `reqwest`
//!
//! `reqwest` is already a workspace dependency (see the root `Cargo.toml`),
//! but only for `npc-llm`'s calls to a configurable, possibly-remote,
//! possibly-TLS OpenAI-compatible endpoint 窶・that's a real HTTP client
//! talking to an arbitrary server over the open internet, which is exactly
//! what justifies carrying `reqwest` and its `rustls` stack there.
//!
//! This module's entire address space is `127.0.0.1`, plaintext, no auth, no
//! redirects, no compression, no proxies 窶・npc-server is the *other half of
//! this same application*, started by the same user on the same machine (or
//! reached over a plain SSH port-forward, per `ws_client.rs`'s doc comment).
//! Pulling `reqwest` into `npc-tui` for that would mean linking its
//! connection-pooling machinery and TLS backend into a terminal client that
//! will only ever open plaintext loopback sockets 窶・the same "don't carry
//! weight you'll never use" call this crate's `Cargo.toml` already makes for
//! `tokio-tungstenite` (TLS features stripped, since `/ws` is also always
//! `ws://`, never `wss://`). A `TcpStream` and a few dozen lines of
//! hand-written request/response framing cost less, in dependency weight and
//! compile time, than justifying a second, differently-configured HTTP
//! client stack living next to `npc-llm`'s.
//!
//! ## The wire-format shortcut this relies on
//!
//! Every request this module sends carries `Connection: close`. npc-server's
//! `axum`/`hyper` stack honors that and closes its side of the socket once
//! the response is fully written 窶・which means "read the stream until EOF"
//! is *exactly* "read one complete response," with no need to parse
//! `Content-Length` or handle `Transfer-Encoding: chunked` ourselves. That
//! shortcut only holds because every endpoint called from here answers with
//! a small, fully-buffered `Json(..)` body (never a stream); if a future
//! caller needs one of the streaming routes (`/api/vrm/file/:file` and
//! friends), it will need its own framing-aware path rather than reusing
//! [`request_json`].
//!
//! ## Typed vs. untyped responses
//!
//! `/api/characters` gets a small local [`CharacterSummary`] mirror, because
//! a screen picking a character genuinely wants typed `id`/`name`/`active`
//! fields. A future config editor should take the opposite approach and keep
//! the document as a raw [`serde_json::Value`]: this crate deliberately has
//! no dependency on `npc-core` (see `protocol.rs`'s module doc for the same
//! reasoning applied to `/ws` frames), and mirroring the entire `Config`
//! struct here would recreate exactly the coupling that decision avoids 窶・
//! quite apart from the round-trip requirement above, which a partial mirror
//! would break by construction.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// How long any single REST call waits for npc-server before giving up.
///
/// 5 seconds, the same ceiling `ws_client.rs`'s reconnect backoff caps out
/// at. A loopback TCP round trip normally completes in well under a
/// millisecond, so this is not sized for the network 窶・it's sized for
/// npc-server being momentarily busy on the same request thread pool as a
/// slow upstream LLM call (e.g. `GET /api/llm/models` blocking on a
/// cold-starting local server) without the *settings screen* itself
/// appearing to hang. The failure mode this exists to prevent is the worst
/// one available to a TUI: a frozen input loop with no way to tell "still
/// working" from "will never come back" apart. Every public function here
/// wraps its request in this timeout and returns a plain `Err` on expiry, so
/// the caller can render it as a dismissable notification instead.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------

/// `GET /api/characters`: every character known in the data dir. Answers
/// with a plain JSON *array* (not wrapped in an object) 窶・mirrors
/// `web/src/lib/api.ts`'s `getCharacters(): Promise<CharacterSummary[]>`
/// exactly, including the field name `active` for "is this the one
/// `character.active_id` currently names."
pub async fn list_characters(addr: SocketAddr) -> Result<Vec<CharacterSummary>> {
    let value = request_json(
        addr,
        "GET",
        "/api/characters",
        None,
        DEFAULT_REQUEST_TIMEOUT,
    )
    .await?;
    serde_json::from_value(value).context("parsing GET /api/characters response")
}

/// `POST /api/characters/{id}/activate`: make `id` the active character.
///
/// Applies immediately, no restart required: the server writes
/// `config.character.active_id` to disk, republishes the full config on the
/// `npc:config` bus topic (so npc-talk's persona hot-reload picks it up),
/// and separately broadcasts a fresh `avatar` `/ws`
/// frame to every connected tab/TUI so the model on screen updates too, in
/// case the newly-active character supplies a different one
/// (`npc-server/src/rest.rs`'s `api_activate_character` / `broadcast_avatar`).
///
/// Returns the server's own acknowledgement verbatim 窶・
/// `{"ok": true, "note": "..."}` 窶・rather than a bare `()`, so a caller that
/// wants to surface the note (or a future non-2xx variant of it) can.
pub async fn activate_character(addr: SocketAddr, id: &str) -> Result<Value> {
    let path = format!("/api/characters/{}/activate", encode_path_segment(id));
    request_json(addr, "POST", &path, None, DEFAULT_REQUEST_TIMEOUT).await
}

/// The `{kind,file}` shape of `npc_core::Avatar`, mirrored the same way
/// `protocol.rs` mirrors `npc-server`'s WS types: field-for-field, no shared
/// dependency.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AvatarRef {
    pub kind: String,
    pub file: String,
}

/// One entry of `GET /api/characters`, mirroring
/// `npc-server/src/rest.rs`'s `CharacterSummary` field-for-field.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CharacterSummary {
    pub id: String,
    pub name: String,
    /// Whether this is the character `config.character.active_id` currently
    /// names.
    pub active: bool,
    /// The character's own avatar assignment (not necessarily the model
    /// shown on screen, if this isn't the active character) 窶・absent
    /// entirely when unset, hence the `default`.
    #[serde(default)]
    pub avatar: Option<AvatarRef>,
}

// ---------------------------------------------------------------------
// Path encoding
// ---------------------------------------------------------------------

/// Percent-encode `raw` for use as one path segment. `npc-server` already
/// rejects a character id containing a path separator, `..`, or `:` before
/// it ever reaches the filesystem (`docs/ARCHITECTURE.md`, "Characters"), so
/// in practice every id this crate ever sends is already
/// URL-path-safe 窶・this exists purely as a defense-in-depth mirror of
/// `web/src/lib/api.ts`'s `encodeURIComponent(id)`, not because a
/// server-accepted id is expected to need it.
fn encode_path_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

// ---------------------------------------------------------------------
// JSON request/response plumbing
// ---------------------------------------------------------------------

/// Send one request and parse its body as JSON, wrapping the whole
/// connect/write/read round trip in `request_timeout`. `Err` covers a
/// connect failure, a timeout, a non-2xx status (with the server's own
/// `{"error": "..."}` message surfaced when present), and a body that isn't
/// valid JSON 窶・a caller only ever has to handle one failure shape.
async fn request_json(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&Value>,
    request_timeout: Duration,
) -> Result<Value> {
    let body_bytes = body
        .map(serde_json::to_vec)
        .transpose()
        .context("serializing request body")?;
    let response = send_request(addr, method, path, body_bytes.as_deref(), request_timeout).await?;
    ensure_success(&response, method, path)?;
    parse_json_body(&response.body)
}

/// Raise `response.status` outside `2xx` into an `Err`. npc-server's
/// `error_response` helper always ships `{"error": "<message>"}`
/// (`npc-server/src/rest.rs`), so that field is preferred when the body
/// parses as JSON; anything else (a proxy in between, or some future route
/// that fails before reaching that helper) falls back to the raw body text
/// rather than silently losing the detail.
fn ensure_success(response: &HttpResponse, method: &str, path: &str) -> Result<()> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    let detail = serde_json::from_slice::<Value>(&response.body)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| String::from_utf8_lossy(&response.body).trim().to_string());
    Err(anyhow!(
        "{method} {path} failed: HTTP {}{}",
        response.status,
        if detail.is_empty() {
            String::new()
        } else {
            format!(" ({detail})")
        }
    ))
}

fn parse_json_body(bytes: &[u8]) -> Result<Value> {
    if bytes.is_empty() {
        // A bodyless 2xx (not currently produced by any endpoint this client
        // calls, but cheap to allow) reads as JSON `null` rather than an
        // error 窶・"no content" is a legitimate success shape, just not one
        // any of today's callers happen to receive.
        return Ok(Value::Null);
    }
    serde_json::from_slice(bytes)
        .with_context(|| format!("parsing JSON response body ({} bytes)", bytes.len()))
}

// ---------------------------------------------------------------------
// Raw HTTP/1.1 over a plain TcpStream
// ---------------------------------------------------------------------

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

/// [`send_request_inner`], bounded by `request_timeout`. Kept as its own
/// wrapper (rather than inlining `tokio::time::timeout` at each call site)
/// so every caller gets the exact same "what actually happened" message on
/// expiry, worded for a human reading the TUI's notification line rather
/// than a stack trace.
async fn send_request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    request_timeout: Duration,
) -> Result<HttpResponse> {
    match tokio::time::timeout(
        request_timeout,
        send_request_inner(addr, method, path, body),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => Err(anyhow!(
            "npc-server did not respond to {method} {path} within {request_timeout:?} \
             (is `tc-npc serve` running and responsive at {addr}?)"
        )),
    }
}

/// Open a fresh connection, write one HTTP/1.1 request, and read the whole
/// response. See the module doc for why `Connection: close` + "read to EOF"
/// is a complete, correct response reader for the small set of endpoints
/// this crate calls, without a `Content-Length`/chunked parser.
async fn send_request_inner(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<HttpResponse> {
    let mut stream = TcpStream::connect(addr)
        .await
        .with_context(|| format!("connecting to npc-server at {addr}"))?;

    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(body) = body {
        head.push_str("Content-Type: application/json\r\n");
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");

    stream
        .write_all(head.as_bytes())
        .await
        .context("writing request head")?;
    if let Some(body) = body {
        stream
            .write_all(body)
            .await
            .context("writing request body")?;
    }
    stream.flush().await.context("flushing request")?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .context("reading response (connection closed before it completed?)")?;

    parse_response(&raw)
}

/// Split a full raw response into a status code and body. The blank line
/// (`\r\n\r\n`) between headers and body is the only thing parsed out of the
/// header block 窶・response headers themselves (`Content-Type`, etc.) are
/// never consulted, since `send_request_inner`'s EOF-terminated read already
/// hands over the complete, correctly-bounded body.
fn parse_response(raw: &[u8]) -> Result<HttpResponse> {
    let sep_at = find_subslice(raw, b"\r\n\r\n").ok_or_else(|| {
        anyhow!(
            "malformed HTTP response: no blank line between headers and body \
             ({} byte(s) received before the connection closed)",
            raw.len()
        )
    })?;
    let head = std::str::from_utf8(&raw[..sep_at]).context("response head is not valid UTF-8")?;
    let status_line = head
        .lines()
        .next()
        .ok_or_else(|| anyhow!("empty response head"))?;
    let status = parse_status_code(status_line)?;
    let body = raw[sep_at + 4..].to_vec();
    Ok(HttpResponse { status, body })
}

/// Pull the numeric status out of a status line like `"HTTP/1.1 200 OK"`.
fn parse_status_code(status_line: &str) -> Result<u16> {
    status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow!("malformed status line: {status_line:?}"))?
        .parse::<u16>()
        .with_context(|| format!("malformed status code in {status_line:?}"))
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;

    use tokio::net::TcpListener;

    /// Bind an ephemeral loopback listener, hand back its address, and spawn
    /// a task that accepts exactly one connection and drives it with
    /// `handler`. Tests `.await` the returned `JoinHandle` after making their
    /// client call so a panic inside `handler` (e.g. an assertion on the
    /// request bytes) surfaces as a normal test failure instead of a
    /// mysterious client-side timeout.
    async fn mock_server<F, Fut>(handler: F) -> (SocketAddr, tokio::task::JoinHandle<()>)
    where
        F: FnOnce(TcpStream) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handler(stream).await;
        });
        (addr, handle)
    }

    /// Read everything the client has sent so far. Requests in these tests
    /// are a handful of bytes over loopback, so this loops on short reads
    /// separated by brief idle gaps rather than implementing a real
    /// server-side HTTP parser: once 100ms passes with nothing new arriving,
    /// the client is assumed to be done writing and waiting on the reply.
    async fn read_full_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match tokio::time::timeout(Duration::from_millis(100), stream.read(&mut chunk)).await {
                Ok(Ok(0)) | Err(_) => break,
                Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
                Ok(Err(_)) => break,
            }
        }
        buf
    }

    async fn respond(mut stream: TcpStream, status_line: &str, body: &str) {
        let response = format!(
            "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.flush().await;
        // Dropping `stream` here closes the socket, which is what makes the
        // client's `read_to_end` (relying on `Connection: close`) return.
    }

    #[tokio::test]
    async fn list_characters_parses_the_array_including_a_missing_avatar() {
        let (addr, server) = mock_server(|mut stream| async move {
            let _ = read_full_request(&mut stream).await;
            respond(
                stream,
                "HTTP/1.1 200 OK",
                r#"[{"id":"c1","name":"Alice","active":true,"avatar":{"kind":"vrm","file":"a.vrm"}},{"id":"c2","name":"Bob","active":false}]"#,
            )
            .await;
        })
        .await;

        let characters = list_characters(addr).await.unwrap();
        assert_eq!(characters.len(), 2);
        assert_eq!(characters[0].id, "c1");
        assert!(characters[0].active);
        assert_eq!(
            characters[0].avatar,
            Some(AvatarRef {
                kind: "vrm".to_string(),
                file: "a.vrm".to_string()
            })
        );
        assert_eq!(characters[1].id, "c2");
        assert!(!characters[1].active);
        assert_eq!(characters[1].avatar, None);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn activate_character_posts_to_the_encoded_path() {
        let (addr, server) = mock_server(|mut stream| async move {
            let request = String::from_utf8(read_full_request(&mut stream).await).unwrap();
            // The space in the id must come back percent-encoded in the
            // request line, proving `encode_path_segment` actually ran.
            assert!(
                request.starts_with("POST /api/characters/odd%20id/activate HTTP/1.1\r\n"),
                "{request}"
            );
            respond(
                stream,
                "HTTP/1.1 200 OK",
                r#"{"ok":true,"note":"persona and avatar switched immediately; no restart required"}"#,
            )
            .await;
        })
        .await;

        let value = activate_character(addr, "odd id").await.unwrap();
        assert_eq!(value["ok"], true);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_404_status_surfaces_the_servers_error_message() {
        let (addr, server) = mock_server(|mut stream| async move {
            let _ = read_full_request(&mut stream).await;
            respond(
                stream,
                "HTTP/1.1 404 Not Found",
                r#"{"error":"character not found: missing"}"#,
            )
            .await;
        })
        .await;

        let err = activate_character(addr, "missing").await.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("404"), "{message}");
        assert!(
            message.contains("character not found: missing"),
            "{message}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_non_json_body_is_a_parse_error_not_a_panic() {
        let (addr, server) = mock_server(|mut stream| async move {
            let _ = read_full_request(&mut stream).await;
            respond(stream, "HTTP/1.1 200 OK", "this is not json").await;
        })
        .await;

        let err = list_characters(addr).await.unwrap_err();
        assert!(
            err.to_string().contains("parsing JSON response body"),
            "{err}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_connection_closed_before_headers_complete_is_an_error_not_a_panic() {
        let (addr, server) = mock_server(|mut stream| async move {
            let _ = read_full_request(&mut stream).await;
            // Half a status line, then hang up 窶・no `\r\n\r\n` ever arrives.
            let _ = stream.write_all(b"HTTP/1.1 200").await;
            let _ = stream.flush().await;
            // Dropping `stream` here closes the connection mid-response.
        })
        .await;

        let err = list_characters(addr).await.unwrap_err();
        assert!(err.to_string().contains("malformed HTTP response"), "{err}");
        server.await.unwrap();
    }

    /// The test this whole module exists to make safe: if npc-server hangs
    /// (or is simply down and something else answers the connect but never
    /// speaks), a REST call must return `Err` instead of blocking the TUI's
    /// event loop forever. The mock here accepts the connection and then
    /// does nothing at all 窶・no response, no close 窶・so the only way this
    /// test finishes is via the client's own timeout firing.
    #[tokio::test]
    async fn request_times_out_when_the_server_never_responds() {
        let (addr, server) = mock_server(|mut stream| async move {
            let _ = read_full_request(&mut stream).await;
            // Never write a response, never drop `stream`: hold the
            // connection open and silent until the test's own client-side
            // timeout below gives up first.
            tokio::time::sleep(Duration::from_secs(2)).await;
        })
        .await;

        let short_timeout = Duration::from_millis(150);
        let err = request_json(addr, "GET", "/api/config", None, short_timeout)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("did not respond"), "{err}");
        assert!(err.to_string().contains("150ms"), "{err}");

        server.await.unwrap();
    }
}
