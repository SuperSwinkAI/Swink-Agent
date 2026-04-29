//! Mistral chat-completions judge client.
//!
//! Mistral's public API is OpenAI-compatible: bearer auth plus
//! `POST /v1/chat/completions` with a `choices[].message.content`
//! response shape. This client mirrors [`crate::xai::XaiJudgeClient`]
//! and [`crate::openai::OpenAiJudgeClient`] so the shared retry,
//! cancellation, and verdict-parsing semantics apply uniformly.

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use swink_agent_eval::judge::{JudgeClient, JudgeError, JudgeVerdict, RetryPolicy};

use crate::client::{block_on_judge, is_retryable, parse_verdict_text, retry_with_cancel};
use crate::util::truncate_http_body;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_TEMPERATURE: f32 = 0.0;

/// Async judge client backed by Mistral's chat-completions endpoint.
#[derive(Clone)]
pub struct MistralJudgeClient {
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

impl std::fmt::Debug for MistralJudgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MistralJudgeClient")
            .field("base_url", &self.inner.base_url)
            .field("model", &self.inner.model)
            .field("temperature", &self.inner.temperature)
            .field("retry_policy", &self.inner.retry_policy)
            .finish_non_exhaustive()
    }
}

impl MistralJudgeClient {
    /// Build a new Mistral judge client.
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
        let body = MistralRequest {
            model: &self.inner.model,
            temperature: self.inner.temperature,
            messages: vec![MistralMessage {
                role: "user",
                content: prompt,
            }],
        };
        let url = format!("{}/v1/chat/completions", self.inner.base_url);

        let response = self
            .inner
            .http
            .post(&url)
            .bearer_auth(&self.inner.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| JudgeError::Transport(format!("mistral request failed: {error}")))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(classify_http_error(status, &text));
        }

        let parsed: MistralResponse = response.json().await.map_err(|error| {
            JudgeError::MalformedResponse(format!("mistral body parse failed: {error}"))
        })?;

        extract_verdict(&parsed)
    }
}

impl JudgeClient for MistralJudgeClient {
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

/// Blocking convenience wrapper around [`MistralJudgeClient`].
#[derive(Clone, Debug)]
pub struct BlockingMistralJudgeClient {
    inner: MistralJudgeClient,
}

impl BlockingMistralJudgeClient {
    /// Wrap an existing [`MistralJudgeClient`].
    #[must_use]
    pub const fn new(inner: MistralJudgeClient) -> Self {
        Self { inner }
    }

    /// Run a single judge call synchronously.
    pub fn judge(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError> {
        let client = self.inner.clone();
        let prompt = prompt.to_string();
        block_on_judge(async move { client.judge(&prompt).await })
    }

    /// Borrow the underlying async client for mixed sync/async call sites.
    #[must_use]
    pub const fn inner(&self) -> &MistralJudgeClient {
        &self.inner
    }
}

// ─── Wire types ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct MistralRequest<'a> {
    model: &'a str,
    temperature: f32,
    messages: Vec<MistralMessage<'a>>,
}

#[derive(Serialize)]
struct MistralMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize, Debug)]
struct MistralResponse {
    #[serde(default)]
    choices: Vec<MistralChoice>,
}

#[derive(Deserialize, Debug)]
struct MistralChoice {
    message: MistralChoiceMessage,
}

#[derive(Deserialize, Debug)]
struct MistralChoiceMessage {
    content: Option<String>,
}

fn classify_http_error(status: StatusCode, body: &str) -> JudgeError {
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        JudgeError::Transport(format!(
            "mistral http {}: {}",
            status.as_u16(),
            truncate_http_body(body)
        ))
    } else {
        JudgeError::Other(format!(
            "mistral http {}: {}",
            status.as_u16(),
            truncate_http_body(body)
        ))
    }
}

fn extract_verdict(response: &MistralResponse) -> Result<JudgeVerdict, JudgeError> {
    let content = response
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_deref())
        .ok_or_else(|| {
            JudgeError::MalformedResponse("mistral choices[0] missing content".into())
        })?;
    parse_verdict_text(content)
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
