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

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("http request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("api error ({status}): {body}")]
    Api { status: u16, body: String },
    #[error("failed to parse response: {0}")]
    Parse(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, LlmError>;

/// Client for an OpenAI-compatible API.
pub struct LlmClient {
    http: reqwest::Client,
    /// A second client with a shorter timeout, used for TTS requests.
    tts_http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl LlmClient {
    /// `base_url` is normalized: trailing `/` trimmed, then `/v1` appended
    /// unless it's already present. `api_key` may be empty, in which case no
    /// `Authorization` header is sent.
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .expect("failed to build reqwest client");
        let tts_http = reqwest::Client::builder()
            .timeout(TTS_TIMEOUT)
            .build()
            .expect("failed to build reqwest client");

        Self {
            http,
            tts_http,
            base_url: normalize_base_url(&base_url.into()),
            api_key: api_key.into(),
        }
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

fn normalize_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
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
