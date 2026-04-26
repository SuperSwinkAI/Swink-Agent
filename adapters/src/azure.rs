//! Azure `OpenAI` / Azure AI Foundry adapter.
//!
//! This adapter targets Azure's OpenAI-v1-compatible chat completions surface.
//! It reuses the shared OAI-compatible SSE parsing from [`openai_compat`] and
//! the shared transport pipeline from [`oai_transport`].

use std::pin::Pin;
use std::time::{Duration, Instant};

use futures::stream::{self, Stream, StreamExt as _};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use swink_agent::{AgentContext, AssistantMessageEvent, ModelSpec, StreamFn, StreamOptions};
use swink_agent_auth::{ExpiringValue, SingleFlightTokenSource};

use crate::classify::{HttpErrorKind, classify_with_overrides};
use crate::oai_transport::{OaiAdapterShell, oai_send_and_parse, prepare_oai_request};

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

/// Response from the Microsoft identity platform token endpoint.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Clone)]
enum TokenAcquireError {
    Auth(String),
    Throttled(String),
    Network(String),
    Other(String),
}

pub struct AzureStreamFn {
    shell: OaiAdapterShell,
    auth: AzureAuth,
    token_source: SingleFlightTokenSource<String, TokenAcquireError>,
    /// Override token endpoint URL (for testing). `None` = use Microsoft default.
    token_endpoint_override: Option<String>,
}

impl AzureStreamFn {
    #[must_use]
    pub fn new(base_url: impl Into<String>, auth: AzureAuth) -> Self {
        let shell_api_key = match &auth {
            AzureAuth::ApiKey(key) => key.clone(),
            AzureAuth::EntraId { .. } => String::new(),
        };

        Self {
            shell: OaiAdapterShell::new_with_path(
                "Azure",
                base_url,
                shell_api_key,
                "/chat/completions",
            ),
            auth,
            token_source: SingleFlightTokenSource::new(REFRESH_MARGIN),
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
        client: reqwest::Client,
        token_url: String,
        client_id: String,
        client_secret: String,
    ) -> Result<ExpiringValue<String>, TokenAcquireError> {
        let params = [
            ("grant_type", "client_credentials".to_string()),
            ("client_id", client_id),
            ("client_secret", client_secret),
            (
                "scope",
                "https://cognitiveservices.azure.com/.default".to_string(),
            ),
        ];

        let resp = client
            .post(&token_url)
            .form(&params)
            .send()
            .await
            .map_err(|e| TokenAcquireError::Network(format!("token request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(match classify_token_endpoint_status(status) {
                Some(HttpErrorKind::Auth) => TokenAcquireError::Auth(format!(
                    "token endpoint auth error (HTTP {status}): {body}"
                )),
                Some(HttpErrorKind::Throttled) => TokenAcquireError::Throttled(format!(
                    "token endpoint rate limit (HTTP {status}): {body}"
                )),
                Some(HttpErrorKind::Network) => TokenAcquireError::Network(format!(
                    "token endpoint server error (HTTP {status}): {body}"
                )),
                None => TokenAcquireError::Other(format!(
                    "token endpoint returned error (HTTP {status}): {body}"
                )),
            });
        }

        let token_resp: TokenResponse = resp.json().await.map_err(|e| {
            TokenAcquireError::Other(format!("failed to parse token response: {e}"))
        })?;

        Ok(ExpiringValue::new(
            token_resp.access_token,
            Instant::now() + Duration::from_secs(token_resp.expires_in),
        ))
    }

    /// Get a valid token, refreshing if necessary.
    async fn get_or_refresh_token(
        &self,
        tenant_id: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<String, TokenAcquireError> {
        let client = self.shell.client().clone();
        let token_url = self.token_url(tenant_id);
        let client_id = client_id.to_string();
        let client_secret = client_secret.to_string();

        self.token_source
            .get_or_refresh(move || {
                Self::acquire_token(client, token_url, client_id, client_secret)
            })
            .await
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
                    .map_err(|e| match e {
                        TokenAcquireError::Auth(message) => AssistantMessageEvent::error_auth(
                            format!("Azure token error: {message}"),
                        ),
                        TokenAcquireError::Throttled(message) => {
                            AssistantMessageEvent::error_throttled(format!(
                                "Azure token error: {message}"
                            ))
                        }
                        TokenAcquireError::Network(message) => {
                            AssistantMessageEvent::error_network(format!(
                                "Azure token error: {message}"
                            ))
                        }
                        TokenAcquireError::Other(message) => {
                            AssistantMessageEvent::error(format!("Azure token error: {message}"))
                        }
                    })?;
                Ok(request.header("Authorization", format!("Bearer {token}")))
            }
        }
    }
}

fn classify_token_endpoint_status(status: u16) -> Option<HttpErrorKind> {
    match status {
        400..=499 if status != 408 && status != 429 => Some(HttpErrorKind::Auth),
        _ => classify_with_overrides(status, &[]),
    }
}

impl std::fmt::Debug for AzureStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureStreamFn")
            .field("base_url", &self.shell.base_url())
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
        let url = azure.shell.chat_completions_url();
        debug!(
            %url,
            model = %model.model_id,
            messages = context.messages.len(),
            "sending Azure request"
        );

        let request = prepare_oai_request(azure.shell.client(), &url, model, context, options);
        let request = match crate::base::race_pre_stream_cancellation(
            &cancellation_token,
            "Azure request cancelled",
            azure.apply_auth(request, options),
        )
        .await
        {
            Ok(r) => r,
            Err(event) => return stream::iter(crate::base::pre_stream_error(event)).left_stream(),
        };

        oai_send_and_parse(
            request,
            azure.shell.provider(),
            cancellation_token,
            options.on_raw_payload.clone(),
            |status, body| {
                if is_content_filter_error(body) {
                    Some(AssistantMessageEvent::error_content_filtered(format!(
                        "Azure content filter blocked request (HTTP {status})"
                    )))
                } else {
                    None
                }
            },
        )
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
