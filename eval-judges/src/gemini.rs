//! Google Gemini `generateContent` judge client.
//!
//! Dispatches a single-turn prompt to
//! `POST /v1beta/models/<model>:generateContent?key=<api_key>` on
//! `generativelanguage.googleapis.com` and returns the parsed JSON verdict.
//! Authentication is via the `?key=` query parameter, consistent with the
//! public Gemini API; bearer-token modes (OAuth, Vertex) pre-sign upstream
//! and pass the resulting token as the api_key value.
//!
//! Errors follow the same classification as the other judge clients:
//! 429 / 5xx → [`JudgeError::Transport`] (retryable), other non-2xx →
//! [`JudgeError::Other`] (terminal), malformed body →
//! [`JudgeError::MalformedResponse`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use swink_agent_eval::judge::{JudgeClient, JudgeError, JudgeVerdict, RetryPolicy};

use crate::client::{is_retryable, parse_verdict_text, retry_with_cancel};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_TEMPERATURE: f32 = 0.0;

/// Async judge client backed by Gemini's `generateContent` endpoint.
#[derive(Clone)]
pub struct GeminiJudgeClient {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    api_key: String,
    model: String,
    temperature: f32,
    retry_policy: RetryPolicy,
    cancel: CancellationToken,
    http: Client,
}

impl Clone for Inner {
    fn clone(&self) -> Self {
        Self {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            model: self.model.clone(),
            temperature: self.temperature,
            retry_policy: self.retry_policy.clone(),
            cancel: self.cancel.clone(),
            http: self.http.clone(),
        }
    }
}

impl std::fmt::Debug for GeminiJudgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiJudgeClient")
            .field("base_url", &self.inner.base_url)
            .field("model", &self.inner.model)
            .field("temperature", &self.inner.temperature)
            .field("retry_policy", &self.inner.retry_policy)
            .finish_non_exhaustive()
    }
}

impl GeminiJudgeClient {
    /// Build a new judge client. `base_url` should be the scheme + host
    /// (e.g. `https://generativelanguage.googleapis.com`); the client
    /// appends `/v1beta/models/<model>:generateContent?key=<api_key>`
    /// when dispatching.
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        let http = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            inner: Arc::new(Inner {
                base_url: base_url.into().trim_end_matches('/').to_string(),
                api_key: api_key.into(),
                model: model.into(),
                temperature: DEFAULT_TEMPERATURE,
                retry_policy: RetryPolicy::default(),
                cancel: CancellationToken::new(),
                http,
            }),
        }
    }

    /// Override sampling temperature. Default: 0.0 for deterministic grading.
    #[must_use]
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        Arc::make_mut(&mut self.inner).temperature = temperature;
        self
    }

    /// Override the shared retry policy.
    #[must_use]
    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        Arc::make_mut(&mut self.inner).retry_policy = policy;
        self
    }

    /// Attach an external cancellation token.
    #[must_use]
    pub fn with_cancellation(mut self, cancel: CancellationToken) -> Self {
        Arc::make_mut(&mut self.inner).cancel = cancel;
        self
    }

    /// Borrow the configured cancellation token.
    #[must_use]
    pub fn cancellation(&self) -> CancellationToken {
        self.inner.cancel.clone()
    }

    async fn dispatch_once(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError> {
        let body = GeminiRequest {
            contents: vec![GeminiContent {
                role: "user",
                parts: vec![GeminiPart { text: prompt }],
            }],
            generation_config: GeminiGenerationConfig {
                temperature: self.inner.temperature,
            },
        };
        let url = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.inner.base_url,
            self.inner.model,
            encode_query_value(&self.inner.api_key)
        );

        let response = self
            .inner
            .http
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| JudgeError::Transport(format!("gemini request failed: {error}")))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(classify_http_error(status, &text));
        }

        let parsed: GeminiResponse = response.json().await.map_err(|error| {
            JudgeError::MalformedResponse(format!("gemini body parse failed: {error}"))
        })?;

        extract_verdict(&parsed)
    }
}

#[async_trait]
impl JudgeClient for GeminiJudgeClient {
    async fn judge(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError> {
        let this = self;
        let prompt = prompt;
        retry_with_cancel(
            &self.inner.retry_policy,
            &self.inner.cancel,
            is_retryable,
            || async move { this.dispatch_once(prompt).await },
        )
        .await
    }
}

/// Blocking convenience wrapper around [`GeminiJudgeClient`].
#[derive(Clone, Debug)]
pub struct BlockingGeminiJudgeClient {
    inner: GeminiJudgeClient,
}

impl BlockingGeminiJudgeClient {
    /// Wrap an existing [`GeminiJudgeClient`].
    #[must_use]
    pub const fn new(inner: GeminiJudgeClient) -> Self {
        Self { inner }
    }

    /// Run a single judge call synchronously on the current Tokio runtime.
    pub fn judge(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError> {
        let client = self.inner.clone();
        let prompt = prompt.to_string();
        tokio::runtime::Handle::current().block_on(async move { client.judge(&prompt).await })
    }

    /// Borrow the underlying async client for mixed sync/async call sites.
    #[must_use]
    pub const fn inner(&self) -> &GeminiJudgeClient {
        &self.inner
    }
}

// ─── Wire types ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct GeminiRequest<'a> {
    contents: Vec<GeminiContent<'a>>,
    #[serde(rename = "generationConfig")]
    generation_config: GeminiGenerationConfig,
}

#[derive(Serialize)]
struct GeminiContent<'a> {
    role: &'a str,
    parts: Vec<GeminiPart<'a>>,
}

#[derive(Serialize)]
struct GeminiPart<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct GeminiGenerationConfig {
    temperature: f32,
}

#[derive(Deserialize, Debug)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
}

#[derive(Deserialize, Debug)]
struct GeminiCandidate {
    #[serde(default)]
    content: Option<GeminiCandidateContent>,
}

#[derive(Deserialize, Debug)]
struct GeminiCandidateContent {
    #[serde(default)]
    parts: Vec<GeminiCandidatePart>,
}

#[derive(Deserialize, Debug)]
struct GeminiCandidatePart {
    #[serde(default)]
    text: Option<String>,
}

fn classify_http_error(status: StatusCode, body: &str) -> JudgeError {
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        JudgeError::Transport(format!(
            "gemini http {}: {}",
            status.as_u16(),
            truncate(body)
        ))
    } else {
        JudgeError::Other(format!(
            "gemini http {}: {}",
            status.as_u16(),
            truncate(body)
        ))
    }
}

fn truncate(body: &str) -> String {
    const LIMIT: usize = 512;
    if body.len() <= LIMIT {
        body.to_string()
    } else {
        format!("{}…", &body[..LIMIT])
    }
}

/// Minimal percent-encoder for the `?key=` query value. Gemini API keys
/// are URL-safe base64-ish strings, but we still escape the reserved
/// characters a caller could slip in (`&`, `=`, `#`, `+`, `%`, and
/// whitespace) to keep the composed URL well-formed without pulling in
/// an extra dependency.
fn encode_query_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn extract_verdict(response: &GeminiResponse) -> Result<JudgeVerdict, JudgeError> {
    let text = response
        .candidates
        .first()
        .and_then(|candidate| candidate.content.as_ref())
        .and_then(|content| content.parts.first())
        .and_then(|part| part.text.as_deref())
        .ok_or_else(|| {
            JudgeError::MalformedResponse(
                "gemini candidates[0].content.parts[0] missing text".into(),
            )
        })?;

    parse_verdict_text(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_429_is_transport() {
        let error = classify_http_error(StatusCode::TOO_MANY_REQUESTS, "slow");
        assert!(matches!(error, JudgeError::Transport(_)));
    }

    #[test]
    fn classify_503_is_transport() {
        let error = classify_http_error(StatusCode::SERVICE_UNAVAILABLE, "down");
        assert!(matches!(error, JudgeError::Transport(_)));
    }

    #[test]
    fn classify_401_is_terminal() {
        let error = classify_http_error(StatusCode::UNAUTHORIZED, "bad key");
        assert!(matches!(error, JudgeError::Other(_)));
    }
}
