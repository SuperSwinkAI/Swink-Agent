//! Ollama judge client.
//!
//! Ollama exposes a local chat API at `POST /api/chat`. No authentication is
//! required — the client relies on network-level access control instead. The
//! response carries the assistant message inside `message.content`, which is
//! fed through the shared verdict parser.

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use swink_agent_eval::judge::{JudgeClient, JudgeError, JudgeVerdict, RetryPolicy};

use crate::client::{BlockingExt, is_retryable, parse_verdict_text, retry_with_cancel};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_TEMPERATURE: f32 = 0.0;

/// Async judge client backed by a local Ollama `/api/chat` endpoint.
#[derive(Clone)]
pub struct OllamaJudgeClient {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
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
            model: self.model.clone(),
            temperature: self.temperature,
            retry_policy: self.retry_policy.clone(),
            cancel: self.cancel.clone(),
            http: self.http.clone(),
        }
    }
}

impl std::fmt::Debug for OllamaJudgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OllamaJudgeClient")
            .field("base_url", &self.inner.base_url)
            .field("model", &self.inner.model)
            .field("temperature", &self.inner.temperature)
            .field("retry_policy", &self.inner.retry_policy)
            .finish_non_exhaustive()
    }
}

impl OllamaJudgeClient {
    /// Build a new Ollama judge client.
    ///
    /// `base_url` is typically `http://localhost:11434`. No API key is needed.
    #[must_use]
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        let http = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            inner: Arc::new(Inner {
                base_url: base_url.into().trim_end_matches('/').to_string(),
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
        let body = OllamaRequest {
            model: &self.inner.model,
            messages: vec![OllamaMessage {
                role: "user",
                content: prompt,
            }],
            stream: false,
        };
        let url = format!("{}/api/chat", self.inner.base_url);

        let response = self
            .inner
            .http
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| JudgeError::Transport(format!("ollama request failed: {error}")))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(classify_http_error(status, &text));
        }

        let parsed: OllamaResponse = response.json().await.map_err(|error| {
            JudgeError::MalformedResponse(format!("ollama body parse failed: {error}"))
        })?;

        extract_verdict(&parsed)
    }
}

impl JudgeClient for OllamaJudgeClient {
    fn judge<'a>(&'a self, prompt: &'a str) -> swink_agent_eval::JudgeFuture<'a> {
        Box::pin(async move {
            let this = self;
            retry_with_cancel(
                &self.inner.retry_policy,
                &self.inner.cancel,
                is_retryable,
                || async move { this.dispatch_once(prompt).await },
            )
            .await
        })
    }
}

/// Blocking convenience wrapper around [`OllamaJudgeClient`].
#[derive(Clone, Debug)]
pub struct BlockingOllamaJudgeClient {
    inner: OllamaJudgeClient,
}

impl BlockingOllamaJudgeClient {
    /// Wrap an existing [`OllamaJudgeClient`].
    #[must_use]
    pub const fn new(inner: OllamaJudgeClient) -> Self {
        Self { inner }
    }

    /// Run a single judge call synchronously on the current Tokio runtime.
    pub fn judge(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError> {
        let client = self.inner.clone();
        let prompt = prompt.to_string();
        async move { client.judge(&prompt).await }.block_on()
    }

    /// Borrow the underlying async client for mixed sync/async call sites.
    #[must_use]
    pub const fn inner(&self) -> &OllamaJudgeClient {
        &self.inner
    }
}

#[derive(Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage<'a>>,
    stream: bool,
}

#[derive(Serialize)]
struct OllamaMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize, Debug)]
struct OllamaResponse {
    message: OllamaAssistantMessage,
}

#[derive(Deserialize, Debug)]
struct OllamaAssistantMessage {
    content: Option<String>,
}

fn classify_http_error(status: StatusCode, body: &str) -> JudgeError {
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        JudgeError::Transport(format!(
            "ollama http {}: {}",
            status.as_u16(),
            truncate(body)
        ))
    } else {
        JudgeError::Other(format!(
            "ollama http {}: {}",
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

fn extract_verdict(response: &OllamaResponse) -> Result<JudgeVerdict, JudgeError> {
    let content =
        response.message.content.as_deref().ok_or_else(|| {
            JudgeError::MalformedResponse("ollama message missing content".into())
        })?;
    parse_verdict_text(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_429_is_transport() {
        let error = classify_http_error(StatusCode::TOO_MANY_REQUESTS, "slow down");
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
