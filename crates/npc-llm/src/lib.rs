//! A small OpenAI-compatible API client shared by the modules that used to
//! talk to an LLM directly from Go (agent-talk, agent-memory, agent-vision,
//! agent-speech). Works against OpenAI itself, Ollama's `/v1` compatibility
//! layer, or any other OpenAI-compatible endpoint.

use std::time::Duration;

use futures_util::{Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod types;
pub use types::*;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const TTS_TIMEOUT: Duration = Duration::from_secs(15);

pub static STATIC_VOICES: &[&str] = &[
    "alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse",
];

/// Environment variable holding a path to an extra PEM bundle of CA
/// certificates to trust, for endpoints behind a private CA that isn't
/// installed in the OS trust store.
pub const EXTRA_CA_ENV: &str = "TC_NPC_CA_BUNDLE";
/// Environment variable that, when set to a truthy value, disables TLS
/// certificate verification entirely. Escape hatch of last resort.
pub const INSECURE_TLS_ENV: &str = "TC_NPC_INSECURE_TLS";

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("http request failed: {}", describe_reqwest_error(.0))]
    Request(#[from] reqwest::Error),
    #[error("api error ({status}): {body}")]
    Api { status: u16, body: String },
    #[error("failed to parse response: {0}")]
    Parse(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, LlmError>;

/// `reasoning_effort` sent on chat requests that don't pick one themselves.
/// Per tc-docs/drafts/llm-settings-common-v1.md §2.3 the parameter is always
/// on the wire — `"none"` is an explicit value the caller chose, not the
/// absence of one.
pub const DEFAULT_REASONING_EFFORT: &str = "none";

/// Client for an OpenAI-compatible API.
pub struct LlmClient {
    http: reqwest::Client,
    /// A second client with a shorter timeout, used for TTS requests.
    tts_http: reqwest::Client,
    base_url: String,
    api_key: String,
    /// Filled into every `ChatRequest` that leaves `reasoning_effort` unset.
    reasoning_effort: String,
}

impl LlmClient {
    /// `base_url` is normalized: trailing `/` trimmed, then `/v1` appended
    /// unless it's already present or the URL selects a mistl room.
    /// `api_key` may be empty, in which case no
    /// `Authorization` header is sent. Chat requests default to
    /// `DEFAULT_REASONING_EFFORT`; see [`LlmClient::with_reasoning_effort`].
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        let http = build_client(DEFAULT_TIMEOUT);
        let tts_http = build_client(TTS_TIMEOUT);

        Self {
            http,
            tts_http,
            base_url: normalize_base_url(&base_url.into()),
            api_key: api_key.into(),
            reasoning_effort: DEFAULT_REASONING_EFFORT.to_string(),
        }
    }

    /// Override the `reasoning_effort` this client sends. An empty string
    /// means "unconfigured" and falls back to `DEFAULT_REASONING_EFFORT` —
    /// there is no way to omit the parameter.
    pub fn with_reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        let effort = effort.into();
        self.reasoning_effort = if effort.is_empty() {
            DEFAULT_REASONING_EFFORT.to_string()
        } else {
            effort
        };
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn auth(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.api_key.is_empty() {
            builder
        } else {
            builder.bearer_auth(&self.api_key)
        }
    }

    /// A request that already carries an effort keeps it; everything else
    /// gets the client's configured one, so no chat call goes out without
    /// the parameter.
    fn apply_reasoning_effort(&self, req: &mut ChatRequest) {
        if req.reasoning_effort.is_none() {
            req.reasoning_effort = Some(self.reasoning_effort.clone());
        }
    }

    async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response> {
        if resp.status().is_success() {
            Ok(resp)
        } else {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Err(LlmError::Api { status, body })
        }
    }

    async fn handle_json<T: DeserializeOwned>(&self, resp: reqwest::Response) -> Result<T> {
        let resp = Self::check_status(resp).await?;
        let text = resp.text().await?;
        serde_json::from_str(&text).map_err(LlmError::Parse)
    }

    /// Non-streaming chat completion (`POST /chat/completions`, `stream: false`).
    pub async fn chat(&self, mut req: ChatRequest) -> Result<ChatResponse> {
        req.stream = Some(false);
        self.apply_reasoning_effort(&mut req);
        let resp = self
            .auth(self.http.post(self.url("/chat/completions")).json(&req))
            .send()
            .await?;
        self.handle_json(resp).await
    }

    /// Streaming chat completion (`stream: true`), yielding text deltas as
    /// they arrive over SSE. Terminates on `data: [DONE]`.
    pub async fn chat_stream(
        &self,
        mut req: ChatRequest,
    ) -> Result<impl Stream<Item = Result<String>>> {
        req.stream = Some(true);
        self.apply_reasoning_effort(&mut req);
        let resp = self
            .auth(self.http.post(self.url("/chat/completions")).json(&req))
            .send()
            .await?;
        let resp = Self::check_status(resp).await?;
        let mut byte_stream = resp.bytes_stream();

        let stream = async_stream::try_stream! {
            let mut buf = String::new();
            while let Some(chunk) = byte_stream.next().await {
                let chunk = chunk.map_err(LlmError::Request)?;
                buf.push_str(&String::from_utf8_lossy(&chunk));

                while let Some(pos) = buf.find('\n') {
                    let line = buf[..pos].trim_end_matches('\r').to_string();
                    buf.drain(..=pos);
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data == "[DONE]" {
                        return;
                    }
                    let value: Value = serde_json::from_str(data).map_err(LlmError::Parse)?;
                    if let Some(delta) = value["choices"][0]["delta"]["content"].as_str() {
                        if !delta.is_empty() {
                            yield delta.to_string();
                        }
                    }
                }
            }
        };

        Ok(stream)
    }

    /// `POST /embeddings`.
    pub async fn embed(&self, model: &str, inputs: Vec<String>) -> Result<Vec<Vec<f32>>> {
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            input: Vec<String>,
        }
        #[derive(Deserialize)]
        struct EmbeddingItem {
            embedding: Vec<f32>,
        }
        #[derive(Deserialize)]
        struct EmbeddingResp {
            data: Vec<EmbeddingItem>,
        }

        let body = Req { model, input: inputs };
        let resp = self
            .auth(self.http.post(self.url("/embeddings")).json(&body))
            .send()
            .await?;
        let parsed: EmbeddingResp = self.handle_json(resp).await?;
        Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
    }

    /// `POST /audio/transcriptions` (multipart), returns the transcribed text.
    pub async fn transcribe(&self, model: &str, wav_bytes: Vec<u8>) -> Result<String> {
        let part = reqwest::multipart::Part::bytes(wav_bytes)
            .file_name("speech.wav")
            .mime_str("audio/wav")?;
        let form = reqwest::multipart::Form::new()
            .text("model", model.to_string())
            .part("file", part);

        let resp = self
            .auth(self.http.post(self.url("/audio/transcriptions")))
            .multipart(form)
            .send()
            .await?;

        #[derive(Deserialize)]
        struct TranscriptResp {
            #[serde(default)]
            text: String,
        }
        let parsed: TranscriptResp = self.handle_json(resp).await?;
        Ok(parsed.text)
    }

    /// `POST /audio/speech`, returns raw audio bytes (WAV).
    pub async fn speak(&self, model: &str, voice: &str, text: &str, speed: f32) -> Result<Vec<u8>> {
        #[derive(Serialize)]
        struct Req<'a> {
            model: &'a str,
            input: &'a str,
            voice: &'a str,
            response_format: &'a str,
            speed: f32,
        }
        let body = Req {
            model,
            input: text,
            voice,
            response_format: "wav",
            speed,
        };
        let resp = self
            .auth(self.tts_http.post(self.url("/audio/speech")).json(&body))
            .send()
            .await?;
        let resp = Self::check_status(resp).await?;
        Ok(resp.bytes().await?.to_vec())
    }

    /// `GET /models`.
    pub async fn list_models(&self) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct ModelItem {
            id: String,
        }
        #[derive(Deserialize)]
        struct ModelsResp {
            #[serde(default)]
            data: Vec<ModelItem>,
        }
        let resp = self.auth(self.http.get(self.url("/models"))).send().await?;
        let parsed: ModelsResp = self.handle_json(resp).await?;
        Ok(parsed.data.into_iter().map(|m| m.id).collect())
    }

    /// `GET /audio/voices`, falling back to `GET /voices`, falling back to a
    /// static list of the standard OpenAI voice names.
    pub async fn list_voices(&self) -> Result<Vec<String>> {
        for path in ["/audio/voices", "/voices"] {
            if let Ok(voices) = self.try_list_voices(path).await {
                if !voices.is_empty() {
                    return Ok(voices);
                }
            }
        }
        Ok(STATIC_VOICES.iter().map(|s| s.to_string()).collect())
    }

    async fn try_list_voices(&self, path: &str) -> Result<Vec<String>> {
        let resp = self.auth(self.http.get(self.url(path))).send().await?;
        let resp = Self::check_status(resp).await?;
        let value: Value = resp.json().await?;
        Ok(extract_voice_names(&value))
    }
}

/// Builds an HTTP client that trusts, in addition to the bundled webpki roots
/// and the OS trust store (see the `rustls-tls-native-roots` note in the root
/// `Cargo.toml`), any CA certificates in the PEM bundle named by
/// [`EXTRA_CA_ENV`]. [`INSECURE_TLS_ENV`] disables verification outright.
///
/// A bad bundle path or an unparseable PEM is logged and skipped rather than
/// fatal: losing one extra CA shouldn't take down endpoints that verify fine
/// against the normal roots.
fn build_client(timeout: Duration) -> reqwest::Client {
    let mut builder = reqwest::Client::builder().timeout(timeout);

    if let Ok(path) = std::env::var(EXTRA_CA_ENV) {
        if !path.trim().is_empty() {
            match std::fs::read(&path) {
                Ok(pem) => match reqwest::Certificate::from_pem_bundle(&pem) {
                    Ok(certs) => {
                        for cert in certs {
                            builder = builder.add_root_certificate(cert);
                        }
                    }
                    Err(err) => {
                        tracing::warn!("{EXTRA_CA_ENV}={path}: not a valid PEM bundle: {err}")
                    }
                },
                Err(err) => tracing::warn!("{EXTRA_CA_ENV}={path}: cannot read: {err}"),
            }
        }
    }

    if is_truthy_env(INSECURE_TLS_ENV) {
        tracing::warn!("{INSECURE_TLS_ENV} is set: TLS certificate verification is DISABLED");
        builder = builder.danger_accept_invalid_certs(true);
    }

    builder.build().expect("failed to build reqwest client")
}

fn is_truthy_env(key: &str) -> bool {
    match std::env::var(key) {
        Ok(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => false,
    }
}

/// `reqwest::Error`'s own `Display` stops at "error sending request for url
/// (…)" and leaves the actual cause — TLS handshake failure, DNS, connection
/// refused — in the `source()` chain, which is exactly the part a user needs
/// when a connection test fails. Flatten the whole chain into the message.
fn describe_reqwest_error(err: &reqwest::Error) -> String {
    let mut out = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        let text = cause.to_string();
        // Skip links that just repeat what we already printed.
        if !out.ends_with(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = cause.source();
    }
    if out.contains("UnknownIssuer") {
        out.push_str(&format!(
            " (hint: the endpoint's certificate is signed by a CA that isn't trusted. \
             Install its root CA in the OS trust store, or point {EXTRA_CA_ENV} at a PEM bundle containing it.)"
        ));
    }
    out
}

fn normalize_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1")
        || trimmed.rsplit_once("/rooms/").is_some_and(|(_, room)| !room.is_empty() && !room.contains('/'))
    {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    }
}

/// Best-effort extraction of voice names from a variety of plausible JSON
/// shapes: a bare array of strings/objects, or `{"voices": [...]}` /
/// `{"data": [...]}` wrappers.
fn extract_voice_names(value: &Value) -> Vec<String> {
    let items: Vec<Value> = if let Some(a) = value.as_array() {
        a.clone()
    } else if let Some(a) = value.get("voices").and_then(|v| v.as_array()) {
        a.clone()
    } else if let Some(a) = value.get("data").and_then(|v| v.as_array()) {
        a.clone()
    } else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|v| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| v.get("name").and_then(Value::as_str).map(str::to_string))
                .or_else(|| v.get("id").and_then(Value::as_str).map(str::to_string))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_base_url() {
        assert_eq!(
            normalize_base_url("http://localhost:11434/v1"),
            "http://localhost:11434/v1"
        );
        assert_eq!(
            normalize_base_url("http://localhost:11434/v1/"),
            "http://localhost:11434/v1"
        );
        assert_eq!(
            normalize_base_url("https://api.openai.com"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            normalize_base_url("https://api.openai.com/"),
            "https://api.openai.com/v1"
        );
    }

    #[test]
    fn fills_in_reasoning_effort() {
        let default_client = LlmClient::new("http://localhost:11434/v1", "");
        let mut req = ChatRequest::new("m", vec![]);
        default_client.apply_reasoning_effort(&mut req);
        assert_eq!(req.reasoning_effort.as_deref(), Some("none"));

        // Empty config value means "unconfigured", not "omit".
        let empty = LlmClient::new("http://localhost:11434/v1", "").with_reasoning_effort("");
        let mut req = ChatRequest::new("m", vec![]);
        empty.apply_reasoning_effort(&mut req);
        assert_eq!(req.reasoning_effort.as_deref(), Some("none"));

        let high = LlmClient::new("http://localhost:11434/v1", "").with_reasoning_effort("high");
        let mut req = ChatRequest::new("m", vec![]);
        high.apply_reasoning_effort(&mut req);
        assert_eq!(req.reasoning_effort.as_deref(), Some("high"));

        // A request that picked its own effort keeps it.
        let mut req = ChatRequest::new("m", vec![]);
        req.reasoning_effort = Some("low".to_string());
        high.apply_reasoning_effort(&mut req);
        assert_eq!(req.reasoning_effort.as_deref(), Some("low"));
    }

    #[test]
    fn extracts_voice_names_from_various_shapes() {
        assert_eq!(
            extract_voice_names(&serde_json::json!(["alloy", "nova"])),
            vec!["alloy", "nova"]
        );
        assert_eq!(
            extract_voice_names(&serde_json::json!({"voices": ["alloy"]})),
            vec!["alloy"]
        );
        assert_eq!(
            extract_voice_names(&serde_json::json!({"data": [{"id": "alloy"}]})),
            vec!["alloy"]
        );
    }
}
