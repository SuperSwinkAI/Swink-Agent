//! AWS Bedrock InvokeModel judge client.
//!
//! Dispatches a single-turn prompt to
//! `POST /model/<model-id>/invoke` on the Bedrock Runtime endpoint and
//! returns the parsed JSON verdict. The wire shape assumed here is the
//! Anthropic-on-Bedrock body (`anthropic_version` + `messages`), which is
//! by far the most common judge configuration; the response content
//! blocks match the Anthropic Messages API so the same
//! [`crate::client::parse_verdict_text`] helper covers verdict parsing.
//!
//! Authentication uses a bearer-style Bedrock API key (the
//! `AWS_BEARER_TOKEN_BEDROCK`-style credential rolled out by Bedrock in
//! 2024). Production deployments that rely on SigV4-signed credentials
//! can pre-sign requests upstream and pass the result as the bearer
//! value; this keeps the judge layer free of a heavy `aws-sdk-*` or
//! `aws-sigv4` dependency.
//!
//! Errors follow the same classification as the Anthropic client:
//! transport / 429 / 5xx → [`JudgeError::Transport`] (retryable),
//! other non-2xx → [`JudgeError::Other`] (terminal), malformed body →
//! [`JudgeError::MalformedResponse`].

use std::sync::Arc;
use std::time::Duration;

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use swink_agent_eval::judge::{JudgeClient, JudgeError, JudgeVerdict, RetryPolicy};

use crate::client::{is_retryable, parse_verdict_text, retry_with_cancel};

const DEFAULT_ANTHROPIC_VERSION: &str = "bedrock-2023-05-31";
const DEFAULT_MAX_TOKENS: u32 = 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Async judge client backed by AWS Bedrock's Runtime InvokeModel endpoint.
#[derive(Clone)]
pub struct BedrockJudgeClient {
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

impl std::fmt::Debug for BedrockJudgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BedrockJudgeClient")
            .field("base_url", &self.inner.base_url)
            .field("model", &self.inner.model)
            .field("anthropic_version", &self.inner.anthropic_version)
            .field("retry_policy", &self.inner.retry_policy)
            .finish_non_exhaustive()
    }
}

impl BedrockJudgeClient {
    /// Build a new judge client. `base_url` should be the scheme + host for
    /// the regional Bedrock Runtime endpoint (e.g.
    /// `https://bedrock-runtime.us-east-1.amazonaws.com`); the client
    /// appends `/model/<model-id>/invoke` when dispatching.
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

    /// Override the `anthropic_version` body field. Default:
    /// `bedrock-2023-05-31`.
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
        let body = BedrockRequest {
            anthropic_version: &self.inner.anthropic_version,
            max_tokens: self.inner.max_tokens,
            messages: vec![BedrockMessage {
                role: "user",
                content: prompt,
            }],
        };
        let url = format!(
            "{}/model/{}/invoke",
            self.inner.base_url,
            urlencode_path(&self.inner.model)
        );

        let response = self
            .inner
            .http
            .post(&url)
            .bearer_auth(&self.inner.api_key)
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| JudgeError::Transport(format!("bedrock request failed: {error}")))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(classify_http_error(status, &text));
        }

        let parsed: BedrockResponse = response.json().await.map_err(|error| {
            JudgeError::MalformedResponse(format!("bedrock body parse failed: {error}"))
        })?;

        extract_verdict(&parsed)
    }
}

impl JudgeClient for BedrockJudgeClient {
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

/// Blocking convenience wrapper around [`BedrockJudgeClient`].
#[derive(Clone, Debug)]
pub struct BlockingBedrockJudgeClient {
    inner: BedrockJudgeClient,
}

impl BlockingBedrockJudgeClient {
    /// Wrap an existing [`BedrockJudgeClient`].
    #[must_use]
    pub const fn new(inner: BedrockJudgeClient) -> Self {
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
    pub const fn inner(&self) -> &BedrockJudgeClient {
        &self.inner
    }
}

// ─── Wire types ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct BedrockRequest<'a> {
    anthropic_version: &'a str,
    max_tokens: u32,
    messages: Vec<BedrockMessage<'a>>,
}

#[derive(Serialize)]
struct BedrockMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize, Debug)]
struct BedrockResponse {
    #[serde(default)]
    content: Vec<BedrockResponseBlock>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum BedrockResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

fn classify_http_error(status: StatusCode, body: &str) -> JudgeError {
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        JudgeError::Transport(format!(
            "bedrock http {}: {}",
            status.as_u16(),
            truncate(body)
        ))
    } else {
        JudgeError::Other(format!(
            "bedrock http {}: {}",
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

/// Minimal path-segment encoder for Bedrock model identifiers. Bedrock
/// model ids can include a provider prefix and colons (e.g.
/// `anthropic.claude-3-5-sonnet-20240620-v1:0`); the colon is the only
/// character we must percent-encode to keep the URL legal.
fn urlencode_path(model: &str) -> String {
    let mut out = String::with_capacity(model.len());
    for ch in model.chars() {
        match ch {
            ':' => out.push_str("%3A"),
            '/' => out.push_str("%2F"),
            _ => out.push(ch),
        }
    }
    out
}

fn extract_verdict(response: &BedrockResponse) -> Result<JudgeVerdict, JudgeError> {
    let text = response
        .content
        .iter()
        .find_map(|b| match b {
            BedrockResponseBlock::Text { text } => Some(text.as_str()),
            BedrockResponseBlock::Other => None,
        })
        .ok_or_else(|| JudgeError::MalformedResponse("no text block in bedrock response".into()))?;

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
    fn classify_500_is_transport() {
        let error = classify_http_error(StatusCode::INTERNAL_SERVER_ERROR, "boom");
        assert!(matches!(error, JudgeError::Transport(_)));
    }

    #[test]
    fn classify_400_is_terminal() {
        let error = classify_http_error(StatusCode::BAD_REQUEST, "nope");
        assert!(matches!(error, JudgeError::Other(_)));
    }

    #[test]
    fn urlencode_colon_in_model_id() {
        let encoded = urlencode_path("anthropic.claude-3-5-sonnet-20240620-v1:0");
        assert!(encoded.contains("%3A"));
        assert!(!encoded.contains(':'));
    }
}
