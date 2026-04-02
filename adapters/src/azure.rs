//! Azure `OpenAI` / Azure AI Foundry adapter.
//!
//! This adapter targets Azure's OpenAI-v1-compatible chat completions surface.
//! It reuses the shared OAI-compatible SSE parsing from [`openai_compat`].

use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use futures::stream::{self, Stream, StreamExt as _};
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

/// Cached `OAuth2` token for Entra ID authentication (used in Phase 5 / US3).
#[allow(dead_code)]
struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

pub struct AzureStreamFn {
    base: AdapterBase,
    auth: AzureAuth,
    #[allow(dead_code)] // Used in Phase 5 (US3) for Entra ID token caching
    token_cache: Arc<RwLock<Option<CachedToken>>>,
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
        AzureAuth::EntraId { .. } => {
            // Entra ID token acquisition will be implemented in Phase 5 (US3).
            // For now, if a runtime api_key override is provided, use it as a Bearer token.
            if let Some(token) = options.api_key.as_deref() {
                request = request.header("Authorization", format!("Bearer {token}"));
            }
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
