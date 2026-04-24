//! Proxy judge client.
//!
//! The proxy client forwards prompts to a generic judge-compatible HTTP
//! endpoint via `POST /judge` (the path is baked into `base_url`). The
//! endpoint returns a raw [`JudgeVerdict`] JSON object, which is parsed
//! directly by the shared verdict parser. Bearer auth is used for
//! authentication.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use swink_agent_eval::judge::{JudgeClient, JudgeError, JudgeVerdict, RetryPolicy};

use crate::client::{BlockingExt, is_retryable, parse_verdict_text, retry_with_cancel};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Async judge client that forwards prompts to a generic proxy endpoint.
///
/// The proxy is responsible for model selection; the client sends only the
/// prompt and receives a raw [`JudgeVerdict`] JSON payload in return.
#[derive(Clone)]
pub struct ProxyJudgeClient {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    api_key: String,
    retry_policy: RetryPolicy,
    cancel: CancellationToken,
    http: Client,
}

impl Clone for Inner {
    fn clone(&self) -> Self {
        Self {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            retry_policy: self.retry_policy.clone(),
            cancel: self.cancel.clone(),
            http: self.http.clone(),
        }
    }
}

impl std::fmt::Debug for ProxyJudgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyJudgeClient")
            .field("base_url", &self.inner.base_url)
            .field("retry_policy", &self.inner.retry_policy)
            .finish_non_exhaustive()
    }
}

impl ProxyJudgeClient {
    /// Build a new proxy judge client.
    ///
    /// `base_url` should include the full path to the judge endpoint, e.g.
    /// `https://proxy.example.com/judge`. No model selection is performed
    /// client-side — the proxy chooses the backend model.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        let http = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            inner: Arc::new(Inner {
                base_url: base_url.into().trim_end_matches('/').to_string(),
                api_key: api_key.into(),
                retry_policy: RetryPolicy::default(),
                cancel: CancellationToken::new(),
                http,
            }),
        }
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
        let body = ProxyRequest { prompt };
        let url = self.inner.base_url.clone();

        let response = self
            .inner
            .http
            .post(&url)
            .bearer_auth(&self.inner.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| JudgeError::Transport(format!("proxy request failed: {error}")))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(classify_http_error(status, &text));
        }

        let text = response.text().await.map_err(|error| {
            JudgeError::MalformedResponse(format!("proxy body read failed: {error}"))
        })?;

        parse_verdict_text(&text)
    }
}

#[async_trait]
impl JudgeClient for ProxyJudgeClient {
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

/// Blocking convenience wrapper around [`ProxyJudgeClient`].
#[derive(Clone, Debug)]
pub struct BlockingProxyJudgeClient {
    inner: ProxyJudgeClient,
}

impl BlockingProxyJudgeClient {
    /// Wrap an existing [`ProxyJudgeClient`].
    #[must_use]
    pub const fn new(inner: ProxyJudgeClient) -> Self {
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
    pub const fn inner(&self) -> &ProxyJudgeClient {
        &self.inner
    }
}

#[derive(Serialize)]
struct ProxyRequest<'a> {
    prompt: &'a str,
}

fn classify_http_error(status: StatusCode, body: &str) -> JudgeError {
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        JudgeError::Transport(format!(
            "proxy http {}: {}",
            status.as_u16(),
            truncate(body)
        ))
    } else {
        JudgeError::Other(format!(
            "proxy http {}: {}",
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
