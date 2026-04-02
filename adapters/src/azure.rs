//! Azure `OpenAI` / Azure AI Foundry adapter.
//!
//! This adapter targets Azure's OpenAI-v1-compatible chat completions surface.
//! It reuses the shared OAI-compatible SSE parsing from [`openai_compat`].

use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use futures::stream::{self, Stream, StreamExt as _};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{AgentContext, ModelSpec};

use crate::base::AdapterBase;
use crate::convert;
use crate::openai_compat::{
    OaiChatRequest, OaiConverter, OaiStreamOptions, build_oai_tools, parse_oai_sse_stream,
};

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
    base: AdapterBase,
    auth: AzureAuth,
    token_cache: Arc<RwLock<Option<CachedToken>>>,
    /// Override token endpoint URL (for testing). `None` = use Microsoft default.
    token_endpoint_override: Option<String>,
}

impl AzureStreamFn {
    #[must_use]
    pub fn new(base_url: impl Into<String>, auth: AzureAuth) -> Self {
        let api_key = match &auth {
            AzureAuth::ApiKey(key) => key.clone(),
            AzureAuth::EntraId { .. } => String::new(),
        };
        Self {
            base: AdapterBase::new(base_url.into().trim_end_matches('/').to_string(), api_key),
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
            .base
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
            let cache = self.token_cache.read().unwrap_or_else(|e| e.into_inner());
            if let Some(cached) = cache.as_ref() {
                if Instant::now() + REFRESH_MARGIN < cached.expires_at {
                    return Ok(cached.access_token.clone());
                }
            }
        }

        // Acquire new token
        let token = self
            .acquire_token(tenant_id, client_id, client_secret)
            .await?;
        let access_token = token.access_token.clone();

        // Update cache
        let mut cache = self.token_cache.write().unwrap_or_else(|e| e.into_inner());
        *cache = Some(token);

        Ok(access_token)
    }

    /// Build the token endpoint URL. Uses override if set, otherwise Microsoft default.
    fn token_url(&self, tenant_id: &str) -> String {
        if let Some(override_url) = &self.token_endpoint_override {
            override_url.clone()
        } else {
            format!("https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token")
        }
    }
}

impl std::fmt::Debug for AzureStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureStreamFn")
            .field("base_url", &self.base.base_url)
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
        let response = match send_request(azure, model, context, options).await {
            Ok(resp) => resp,
            Err(event) => return stream::iter(vec![event]).left_stream(),
        };

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status = code, "Azure HTTP error");
            let event = crate::classify::error_event_from_status(code, &body, "Azure");
            return stream::iter(vec![event]).left_stream();
        }

        parse_oai_sse_stream(response, cancellation_token, "Azure").right_stream()
    })
    .flatten()
}

async fn send_request(
    azure: &AzureStreamFn,
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
) -> Result<reqwest::Response, AssistantMessageEvent> {
    let url = format!("{}/chat/completions", azure.base.base_url);
    debug!(
        %url,
        model = %model.model_id,
        messages = context.messages.len(),
        "sending Azure request"
    );

    let messages =
        convert::convert_messages::<OaiConverter>(&context.messages, &context.system_prompt);

    let (tools, tool_choice) = build_oai_tools(&context.tools);

    let body = OaiChatRequest {
        model: model.model_id.clone(),
        messages,
        stream: true,
        stream_options: OaiStreamOptions {
            include_usage: true,
        },
        temperature: options.temperature,
        max_tokens: options.max_tokens,
        tools,
        tool_choice,
    };

    let mut request = azure.base.client.post(&url).json(&body);

    match &azure.auth {
        AzureAuth::ApiKey(key) => {
            let api_key = options.api_key.as_deref().unwrap_or(key);
            request = request.header("api-key", api_key);
        }
        AzureAuth::EntraId {
            tenant_id,
            client_id,
            client_secret,
        } => {
            let token = azure
                .get_or_refresh_token(tenant_id, client_id, client_secret)
                .await
                .map_err(|e| {
                    AssistantMessageEvent::error_network(format!("Azure token error: {e}"))
                })?;
            request = request.header("Authorization", format!("Bearer {token}"));
        }
    }

    request
        .send()
        .await
        .map_err(|e| AssistantMessageEvent::error_network(format!("Azure connection error: {e}")))
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AzureStreamFn>();
};
