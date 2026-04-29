//! Anthropic Messages judge client.
//!
//! Wraps a plain `reqwest::Client` around the Anthropic `/v1/messages`
//! endpoint and dispatches a single-turn user prompt, returning the JSON
//! verdict the judge template is expected to emit. The full streaming /
//! tool-use adapter surface in `swink-agent-adapters` is deliberately
//! bypassed: judge calls need one request and one response, not the
//! SSE / tool-use state machine.
//!
//! Error mapping (see FR-004 + research.md §R-002):
//!
//! * Transport failures → [`JudgeError::Transport`].
//! * HTTP 429 / 5xx → [`JudgeError::Transport`] so the shared retry helper
//!   in [`crate::client`] classifies them as retryable.
//! * Non-2xx / non-retryable 4xx → [`JudgeError::Other`] (terminal).
//! * Response body that fails JSON verdict parsing →
//!   [`JudgeError::MalformedResponse`].
//! * Cancellation (via the supplied [`CancellationToken`]) surfaces as
//!   [`JudgeError::Other`] with a cancellation tag.

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use swink_agent_eval::judge::{JudgeClient, JudgeError, JudgeVerdict, RetryPolicy};

use crate::client::{block_on_judge, is_retryable, parse_verdict_text, retry_with_cancel};
use crate::util::truncate_http_body;

const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Async judge client backed by the Anthropic Messages API.
///
/// Constructed via [`AnthropicJudgeClient::new`]; clone is cheap (HTTP
/// client + `Arc`-held config).
#[derive(Clone)]
pub struct AnthropicJudgeClient {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    api_key: String,
    model: String,
    anthropic_version: String,
    max_tokens: u32,
    retry_policy: RetryPolicy,
    cancel: CancellationToken,
    http: Client,
}

impl std::fmt::Debug for AnthropicJudgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicJudgeClient")
            .field("base_url", &self.inner.base_url)
            .field("model", &self.inner.model)
            .field("anthropic_version", &self.inner.anthropic_version)
            .field("retry_policy", &self.inner.retry_policy)
            .finish_non_exhaustive()
    }
}

impl AnthropicJudgeClient {
    /// Build a new judge client pointed at `base_url` (e.g.
    /// `https://api.anthropic.com`). `model` is the Anthropic model id
    /// used for every judge call.
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
                anthropic_version: DEFAULT_ANTHROPIC_VERSION.to_string(),
                max_tokens: DEFAULT_MAX_TOKENS,
                retry_policy: RetryPolicy::default(),
                cancel: CancellationToken::new(),
                http,
            }),
        }
    }

    /// Override the `anthropic-version` header.
    #[must_use]
    pub fn with_anthropic_version(mut self, version: impl Into<String>) -> Self {
        Arc::make_mut(&mut self.inner).anthropic_version = version.into();
        self
    }

    /// Override the `max_tokens` request field. Default: 1024.
    #[must_use]
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        Arc::make_mut(&mut self.inner).max_tokens = max_tokens;
        self
    }

    /// Override the shared retry policy.
    #[must_use]
    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        Arc::make_mut(&mut self.inner).retry_policy = policy;
        self
    }

    /// Attach an external cancellation token. The default client owns an
    /// internal token that is never cancelled automatically.
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
        let body = AnthropicRequest {
            model: &self.inner.model,
            max_tokens: self.inner.max_tokens,
            messages: vec![AnthropicMessage {
                role: "user",
                content: prompt,
            }],
        };

        let url = format!("{}/v1/messages", self.inner.base_url);

        let resp = self
            .inner
            .http
            .post(&url)
            .header("x-api-key", &self.inner.api_key)
            .header("anthropic-version", &self.inner.anthropic_version)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| JudgeError::Transport(format!("anthropic request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(classify_http_error(status, &text));
        }

        let parsed: AnthropicResponse = resp.json().await.map_err(|e| {
            JudgeError::MalformedResponse(format!("anthropic body parse failed: {e}"))
        })?;

        extract_verdict(&parsed)
    }
}

// Clone requires `Inner` be clone-on-write via Arc::make_mut; provide impl.
impl Clone for Inner {
    fn clone(&self) -> Self {
        Self {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            model: self.model.clone(),
            anthropic_version: self.anthropic_version.clone(),
            max_tokens: self.max_tokens,
            retry_policy: self.retry_policy.clone(),
            cancel: self.cancel.clone(),
            http: self.http.clone(),
        }
    }
}

impl JudgeClient for AnthropicJudgeClient {
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

/// Blocking convenience wrapper around [`AnthropicJudgeClient`] for callers
/// that cannot spin a Tokio runtime themselves.
///
/// Safe to call outside Tokio or from a Tokio runtime; the shared helper owns
/// a runtime when the ambient runtime cannot be blocked directly.
#[derive(Clone, Debug)]
pub struct BlockingAnthropicJudgeClient {
    inner: AnthropicJudgeClient,
}

impl BlockingAnthropicJudgeClient {
    /// Wrap an existing [`AnthropicJudgeClient`].
    #[must_use]
    pub const fn new(inner: AnthropicJudgeClient) -> Self {
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
    pub const fn inner(&self) -> &AnthropicJudgeClient {
        &self.inner
    }
}

// ─── Wire types ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<AnthropicMessage<'a>>,
}

#[derive(Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize, Debug)]
struct AnthropicResponse {
    #[serde(default)]
    content: Vec<AnthropicResponseBlock>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum AnthropicResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

fn classify_http_error(status: StatusCode, body: &str) -> JudgeError {
    // 429 and 5xx are retryable per R-002.
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        JudgeError::Transport(format!(
            "anthropic http {}: {}",
            status.as_u16(),
            truncate_http_body(body)
        ))
    } else {
        JudgeError::Other(format!(
            "anthropic http {}: {}",
            status.as_u16(),
            truncate_http_body(body)
        ))
    }
}

/// Parse the judge verdict from the first text block in the Anthropic
/// response.
///
/// Accepts either a bare JSON object with the verdict fields or the same
/// object wrapped in a markdown code fence. Anything else is surfaced as
/// [`JudgeError::MalformedResponse`] so the retry helper classifies it as
/// terminal.
fn extract_verdict(resp: &AnthropicResponse) -> Result<JudgeVerdict, JudgeError> {
    let text = resp
        .content
        .iter()
        .find_map(|b| match b {
            AnthropicResponseBlock::Text { text } => Some(text.as_str()),
            AnthropicResponseBlock::Other => None,
        })
        .ok_or_else(|| {
            JudgeError::MalformedResponse("no text block in anthropic response".into())
        })?;

    parse_verdict_text(text)
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
    fn classify_500_is_transport() {
        let err = classify_http_error(StatusCode::INTERNAL_SERVER_ERROR, "boom");
        assert!(matches!(err, JudgeError::Transport(_)));
    }

    #[test]
    fn classify_400_is_terminal() {
        let err = classify_http_error(StatusCode::BAD_REQUEST, "nope");
        assert!(matches!(err, JudgeError::Other(_)));
    }
}
