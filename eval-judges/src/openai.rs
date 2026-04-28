//! OpenAI Chat Completions judge client.
//!
//! Mirrors the shape of [`crate::anthropic::AnthropicJudgeClient`]: a
//! single-request, single-response POST to the provider plus shared
//! retry / cancellation via [`crate::client::retry_with_cancel`]. The
//! full SSE / tool-use pipeline in `swink-agent-adapters::openai` is
//! deliberately bypassed — a judge call does not need a streaming
//! assistant loop.
//!
//! The exported name preserves the existing `OpenAi…` casing used by
//! `eval-judges/src/lib.rs`; a `type OpenAIJudgeClient =
//! OpenAiJudgeClient` alias is provided so spec documentation that uses
//! `OpenAIJudgeClient` also compiles.

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use swink_agent_eval::judge::{JudgeClient, JudgeError, JudgeVerdict, RetryPolicy};

use crate::client::{is_retryable, parse_verdict_text, retry_with_cancel};
use crate::util::truncate_http_body;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_TEMPERATURE: f32 = 0.0;

/// Async judge client backed by an OpenAI-compatible Chat Completions
/// endpoint.
///
/// Works for OpenAI itself and any OpenAI-compatible provider with a
/// `Bearer <api_key>` auth scheme and the `/v1/chat/completions`
/// endpoint shape.
#[derive(Clone)]
pub struct OpenAiJudgeClient {
    inner: Arc<Inner>,
}

/// Alias for spec-doc callers that use `OpenAI` capitalization.
#[allow(dead_code)]
pub type OpenAIJudgeClient = OpenAiJudgeClient;

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

impl std::fmt::Debug for OpenAiJudgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiJudgeClient")
            .field("base_url", &self.inner.base_url)
            .field("model", &self.inner.model)
            .field("temperature", &self.inner.temperature)
            .field("retry_policy", &self.inner.retry_policy)
            .finish_non_exhaustive()
    }
}

impl OpenAiJudgeClient {
    /// Build a new judge client. `base_url` should be the scheme + host
    /// (e.g. `https://api.openai.com`); the client appends
    /// `/v1/chat/completions` when dispatching.
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

    /// Override sampling temperature. Default: 0.0 (deterministic
    /// grading).
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
        let body = OpenAiRequest {
            model: &self.inner.model,
            temperature: self.inner.temperature,
            messages: vec![OpenAiMessage {
                role: "user",
                content: prompt,
            }],
        };
        let url = format!("{}/v1/chat/completions", self.inner.base_url);

        let resp = self
            .inner
            .http
            .post(&url)
            .bearer_auth(&self.inner.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| JudgeError::Transport(format!("openai request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(classify_http_error(status, &text));
        }

        let parsed: OpenAiResponse = resp
            .json()
            .await
            .map_err(|e| JudgeError::MalformedResponse(format!("openai body parse failed: {e}")))?;

        extract_verdict(&parsed)
    }
}

impl JudgeClient for OpenAiJudgeClient {
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

/// Blocking convenience wrapper around [`OpenAiJudgeClient`].
#[derive(Clone, Debug)]
pub struct BlockingOpenAiJudgeClient {
    inner: OpenAiJudgeClient,
}

/// Alias for spec-doc callers that use `OpenAI` capitalization.
#[allow(dead_code)]
pub type BlockingOpenAIJudgeClient = BlockingOpenAiJudgeClient;

impl BlockingOpenAiJudgeClient {
    /// Wrap an existing [`OpenAiJudgeClient`].
    #[must_use]
    pub const fn new(inner: OpenAiJudgeClient) -> Self {
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
    pub const fn inner(&self) -> &OpenAiJudgeClient {
        &self.inner
    }
}

// ─── Wire types ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct OpenAiRequest<'a> {
    model: &'a str,
    temperature: f32,
    messages: Vec<OpenAiMessage<'a>>,
}

#[derive(Serialize)]
struct OpenAiMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize, Debug)]
struct OpenAiResponse {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize, Debug)]
struct OpenAiChoice {
    message: OpenAiChoiceMessage,
}

#[derive(Deserialize, Debug)]
struct OpenAiChoiceMessage {
    content: Option<String>,
}

fn classify_http_error(status: StatusCode, body: &str) -> JudgeError {
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        JudgeError::Transport(format!(
            "openai http {}: {}",
            status.as_u16(),
            truncate_http_body(body)
        ))
    } else {
        JudgeError::Other(format!(
            "openai http {}: {}",
            status.as_u16(),
            truncate_http_body(body)
        ))
    }
}

fn extract_verdict(resp: &OpenAiResponse) -> Result<JudgeVerdict, JudgeError> {
    let content = resp
        .choices
        .first()
        .and_then(|c| c.message.content.as_deref())
        .ok_or_else(|| JudgeError::MalformedResponse("openai choices[0] missing content".into()))?;
    parse_verdict_text(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_429_is_transport() {
        let err = classify_http_error(StatusCode::TOO_MANY_REQUESTS, "rate");
        assert!(matches!(err, JudgeError::Transport(_)));
    }

    #[test]
    fn classify_503_is_transport() {
        let err = classify_http_error(StatusCode::SERVICE_UNAVAILABLE, "down");
        assert!(matches!(err, JudgeError::Transport(_)));
    }

    #[test]
    fn classify_401_is_terminal() {
        let err = classify_http_error(StatusCode::UNAUTHORIZED, "bad key");
        assert!(matches!(err, JudgeError::Other(_)));
    }
}
