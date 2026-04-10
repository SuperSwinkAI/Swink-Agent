//! Azure `OpenAI` / Azure AI Foundry adapter.
//!
//! This adapter targets Azure's OpenAI-v1-compatible chat completions surface.
//! It reuses the shared OAI-compatible SSE parsing from [`openai_compat`] and
//! the shared transport pipeline from [`oai_transport`].

use std::pin::Pin;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant};

use futures::stream::{self, Stream, StreamExt as _};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use swink_agent::{AgentContext, AssistantMessageEvent, ModelSpec, StreamFn, StreamOptions};

use crate::oai_transport::{oai_send_and_parse, prepare_oai_request};

/// Authentication method for Azure `OpenAI` deployments.
#[derive(Clone)]
pub enum AzureAuth {
    /// API key authentication via the `api-key` header.
    ApiKey(String),
    /// Azure AD / Entra ID `OAuth2` client credentials flow.
    EntraId {
        tenant_id: String,
        client_id: String,
        client_secret: String,
    },
}

impl std::fmt::Debug for AzureAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey(_) => f.debug_tuple("ApiKey").field(&"[REDACTED]").finish(),
            Self::EntraId { .. } => f
                .debug_struct("EntraId")
                .field("tenant_id", &"[REDACTED]")
                .field("client_id", &"[REDACTED]")
                .field("client_secret", &"[REDACTED]")
                .finish(),
        }
    }
}

/// Refresh tokens proactively 5 minutes before expiry.
const REFRESH_MARGIN: Duration = Duration::from_secs(300);

/// Cached `OAuth2` token for Entra ID authentication.
struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

/// Response from the Microsoft identity platform token endpoint.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

pub struct AzureStreamFn {
    client: reqwest::Client,
    base_url: String,
    auth: AzureAuth,
    token_cache: Arc<RwLock<Option<CachedToken>>>,
    /// Override token endpoint URL (for testing). `None` = use Microsoft default.
    token_endpoint_override: Option<String>,
}

impl AzureStreamFn {
    #[must_use]
    pub fn new(base_url: impl Into<String>, auth: AzureAuth) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            auth,
            token_cache: Arc::new(RwLock::new(None)),
            token_endpoint_override: None,
        }
    }

    /// Set a custom token endpoint URL (for testing with wiremock).
    #[must_use]
    pub fn with_token_endpoint(mut self, url: impl Into<String>) -> Self {
        self.token_endpoint_override = Some(url.into());
        self
    }
}

impl AzureStreamFn {
    /// Acquire a fresh token from the Microsoft identity platform.
    async fn acquire_token(
        &self,
        tenant_id: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<CachedToken, String> {
        let token_url = self.token_url(tenant_id);
        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("scope", "https://cognitiveservices.azure.com/.default"),
        ];

        let resp = self
            .client
            .post(&token_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| format!("token request failed: {e}"))?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("token endpoint returned error: {body}"));
        }

        let token_resp: TokenResponse = resp
            .json()
            .await
            .map_err(|e| format!("failed to parse token response: {e}"))?;

        Ok(CachedToken {
            access_token: token_resp.access_token,
            expires_at: Instant::now() + Duration::from_secs(token_resp.expires_in),
        })
    }

    /// Get a valid token, refreshing if necessary.
    async fn get_or_refresh_token(
        &self,
        tenant_id: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<String, String> {
        // Check cache first
        {
            let cache = self
                .token_cache
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(cached) = cache.as_ref()
                && Instant::now() + REFRESH_MARGIN < cached.expires_at
            {
                return Ok(cached.access_token.clone());
            }
        }

        // Acquire new token
        let token = self
            .acquire_token(tenant_id, client_id, client_secret)
            .await?;
        let access_token = token.access_token.clone();

        // Update cache
        *self
            .token_cache
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(token);

        Ok(access_token)
    }

    /// Build the token endpoint URL. Uses override if set, otherwise Microsoft default.
    fn token_url(&self, tenant_id: &str) -> String {
        self.token_endpoint_override.as_ref().map_or_else(
            || format!("https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token"),
            Clone::clone,
        )
    }

    /// Apply Azure-specific auth headers to the request builder.
    async fn apply_auth(
        &self,
        request: reqwest::RequestBuilder,
        options: &StreamOptions,
    ) -> Result<reqwest::RequestBuilder, AssistantMessageEvent> {
        match &self.auth {
            AzureAuth::ApiKey(key) => {
                let api_key = options.api_key.as_deref().unwrap_or(key);
                Ok(request.header("api-key", api_key))
            }
            AzureAuth::EntraId {
                tenant_id,
                client_id,
                client_secret,
            } => {
                let token = self
                    .get_or_refresh_token(tenant_id, client_id, client_secret)
                    .await
                    .map_err(|e| {
                        AssistantMessageEvent::error_network(format!("Azure token error: {e}"))
                    })?;
                Ok(request.header("Authorization", format!("Bearer {token}")))
            }
        }
    }
}

impl std::fmt::Debug for AzureStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureStreamFn")
            .field("base_url", &self.base_url)
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

impl StreamFn for AzureStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(azure_stream(
            self,
            model,
            context,
            options,
            cancellation_token,
        ))
    }
}

fn azure_stream<'a>(
    azure: &'a AzureStreamFn,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        let url = format!("{}/chat/completions", azure.base_url);
        debug!(
            %url,
            model = %model.model_id,
            messages = context.messages.len(),
            "sending Azure request"
        );

        let request = prepare_oai_request(&azure.client, &url, model, context, options);
        let request = match azure.apply_auth(request, options).await {
            Ok(r) => r,
            Err(event) => return stream::iter(vec![event]).left_stream(),
        };

        oai_send_and_parse(request, "Azure", cancellation_token, |status, body| {
            if is_content_filter_error(body) {
                Some(AssistantMessageEvent::error_content_filtered(format!(
                    "Azure content filter blocked request (HTTP {status})"
                )))
            } else {
                None
            }
        })
        .right_stream()
    })
    .flatten()
}

/// Check if an HTTP error body contains an Azure content filter violation.
///
/// Azure returns `error.code: "ContentFilterBlocked"` when the request
/// or response triggers content safety filters.
fn is_content_filter_error(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error")?.get("code")?.as_str().map(String::from))
        .is_some_and(|code| code == "ContentFilterBlocked")
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AzureStreamFn>();
};
