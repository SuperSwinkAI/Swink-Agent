//! Azure OpenAI judge client.
//!
//! Azure's chat-completions surface is OpenAI-compatible once the caller
//! provides a deployment-scoped base URL and an `api-version` query string.
//! Requests use the `api-key` header rather than bearer auth, but otherwise
//! the verdict parsing, retry, and cancellation semantics match the other
//! OAI-style judge clients in this crate.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use swink_agent_eval::judge::{JudgeClient, JudgeError, JudgeVerdict, RetryPolicy};

use crate::client::{BlockingExt, is_retryable, parse_verdict_text, retry_with_cancel};

const DEFAULT_API_VERSION: &str = "2024-10-21";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_TEMPERATURE: f32 = 0.0;

/// Async judge client backed by the Azure OpenAI chat-completions endpoint.
///
/// `base_url` should include the deployment path, for example:
/// `https://example.openai.azure.com/openai/deployments/gpt-4o`.
#[derive(Clone)]
pub struct AzureJudgeClient {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    api_key: String,
    model: String,
    api_version: String,
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
            api_version: self.api_version.clone(),
            temperature: self.temperature,
            retry_policy: self.retry_policy.clone(),
            cancel: self.cancel.clone(),
            http: self.http.clone(),
        }
    }
}

impl std::fmt::Debug for AzureJudgeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureJudgeClient")
            .field("base_url", &self.inner.base_url)
            .field("model", &self.inner.model)
            .field("api_version", &self.inner.api_version)
            .field("temperature", &self.inner.temperature)
            .field("retry_policy", &self.inner.retry_policy)
            .finish_non_exhaustive()
    }
}

impl AzureJudgeClient {
    /// Build a new Azure judge client.
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
                api_version: DEFAULT_API_VERSION.to_string(),
                temperature: DEFAULT_TEMPERATURE,
                retry_policy: RetryPolicy::default(),
                cancel: CancellationToken::new(),
                http,
            }),
        }
    }

    /// Override the Azure OpenAI `api-version` query parameter.
    #[must_use]
    pub fn with_api_version(mut self, api_version: impl Into<String>) -> Self {
        Arc::make_mut(&mut self.inner).api_version = api_version.into();
        self
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
        let body = AzureRequest {
            model: &self.inner.model,
            temperature: self.inner.temperature,
            messages: vec![AzureMessage {
                role: "user",
                content: prompt,
            }],
        };
        let url = format!(
            "{}/chat/completions?api-version={}",
            self.inner.base_url, self.inner.api_version
        );

        let response = self
            .inner
            .http
            .post(&url)
            .header("api-key", &self.inner.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|error| JudgeError::Transport(format!("azure request failed: {error}")))?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(classify_http_error(status, &text));
        }

        let parsed: AzureResponse = response.json().await.map_err(|error| {
            JudgeError::MalformedResponse(format!("azure body parse failed: {error}"))
        })?;

        extract_verdict(&parsed)
    }
}

#[async_trait]
impl JudgeClient for AzureJudgeClient {
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

/// Blocking convenience wrapper around [`AzureJudgeClient`].
#[derive(Clone, Debug)]
pub struct BlockingAzureJudgeClient {
    inner: AzureJudgeClient,
}

impl BlockingAzureJudgeClient {
    /// Wrap an existing [`AzureJudgeClient`].
    #[must_use]
    pub const fn new(inner: AzureJudgeClient) -> Self {
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
    pub const fn inner(&self) -> &AzureJudgeClient {
        &self.inner
    }
}

#[derive(Serialize)]
struct AzureRequest<'a> {
    model: &'a str,
    temperature: f32,
    messages: Vec<AzureMessage<'a>>,
}

#[derive(Serialize)]
struct AzureMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize, Debug)]
struct AzureResponse {
    #[serde(default)]
    choices: Vec<AzureChoice>,
}

#[derive(Deserialize, Debug)]
struct AzureChoice {
    message: AzureChoiceMessage,
}

#[derive(Deserialize, Debug)]
struct AzureChoiceMessage {
    content: Option<String>,
}

fn classify_http_error(status: StatusCode, body: &str) -> JudgeError {
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        JudgeError::Transport(format!(
            "azure http {}: {}",
            status.as_u16(),
            truncate(body)
        ))
    } else {
        JudgeError::Other(format!(
            "azure http {}: {}",
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

fn extract_verdict(response: &AzureResponse) -> Result<JudgeVerdict, JudgeError> {
    let content = response
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_deref())
        .ok_or_else(|| JudgeError::MalformedResponse("azure choices[0] missing content".into()))?;
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
    fn extract_verdict_requires_choice_content() {
        let error = extract_verdict(&AzureResponse {
            choices: vec![AzureChoice {
                message: AzureChoiceMessage { content: None },
            }],
        })
        .expect_err("missing content must fail");
        assert!(matches!(error, JudgeError::MalformedResponse(_)));
    }
}
