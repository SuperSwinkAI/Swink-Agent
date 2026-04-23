//! Shared OpenAI-compatible transport layer.
//!
//! Provides [`prepare_oai_request`] for building standard request bodies and
//! [`oai_send_and_parse`] for the shared HTTP send → error classification →
//! SSE parsing pipeline. Together they eliminate the duplicated request/send/
//! status/parse shell across OpenAI-protocol adapters (`OpenAI`, Azure, Mistral,
//! xAI).
//!
//! Adapters that need provider-specific hooks (Azure auth, Mistral message
//! ordering, etc.) apply those before or after calling into this module.

#[cfg(feature = "openai-compat")]
use std::pin::Pin;

use futures::stream::{self, Stream, StreamExt as _};
#[cfg(feature = "openai-compat")]
use tokio_util::sync::CancellationToken;
#[cfg(feature = "openai-compat")]
use tracing::debug;
use tracing::warn;

#[cfg(feature = "mistral")]
use serde::Serialize;

use swink_agent::{AgentContext, AssistantMessageEvent, ModelSpec, StreamOptions};

use crate::base::AdapterBase;
use crate::convert;
use crate::openai_compat::{
    OaiChatRequest, OaiConverter, OaiStreamOptions, build_oai_tools, parse_oai_sse_stream,
};

/// Shared shell for Bearer-auth OpenAI-compatible adapters.
///
/// Fully standard adapters can delegate their entire `stream()` implementation
/// to this type, while adapters with provider-specific request normalization
/// can still reuse the shared constructor, debug redaction, endpoint assembly,
/// and API-key override handling.
pub struct OaiAdapterShell {
    provider: &'static str,
    base: AdapterBase,
    chat_completions_path: &'static str,
}

impl OaiAdapterShell {
    #[cfg(any(feature = "openai-compat", feature = "mistral"))]
    pub(crate) fn new(
        provider: &'static str,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            provider,
            base: AdapterBase::new(base_url, api_key),
            chat_completions_path: "/v1/chat/completions",
        }
    }

    #[cfg(any(test, feature = "azure"))]
    pub(crate) fn new_with_path(
        provider: &'static str,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        chat_completions_path: &'static str,
    ) -> Self {
        Self {
            provider,
            base: AdapterBase::new(base_url, api_key),
            chat_completions_path,
        }
    }

    #[cfg(any(test, feature = "azure"))]
    pub(crate) fn base_url(&self) -> &str {
        &self.base.base_url
    }

    #[cfg(feature = "azure")]
    pub(crate) const fn client(&self) -> &reqwest::Client {
        &self.base.client
    }

    #[cfg(any(feature = "openai-compat", feature = "mistral"))]
    pub(crate) fn fmt_debug(
        &self,
        name: &'static str,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        f.debug_struct(name)
            .field("base_url", &self.base.base_url)
            .field("api_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }

    #[cfg(any(feature = "azure", feature = "mistral"))]
    pub(crate) const fn provider(&self) -> &'static str {
        self.provider
    }

    pub(crate) fn chat_completions_url(&self) -> String {
        format!("{}{}", self.base.base_url, self.chat_completions_path)
    }

    #[cfg(any(feature = "openai-compat", feature = "mistral"))]
    pub(crate) fn api_key<'a>(&'a self, options: &'a StreamOptions) -> &'a str {
        options.api_key.as_deref().unwrap_or(&self.base.api_key)
    }

    #[cfg(feature = "mistral")]
    pub(crate) fn post_json_request<T: Serialize>(
        &self,
        url: &str,
        body: &T,
        options: &StreamOptions,
    ) -> reqwest::RequestBuilder {
        self.base
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key(options)))
            .json(body)
    }

    #[cfg(feature = "openai-compat")]
    pub(crate) fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let url = self.chat_completions_url();

        debug!(
            provider = self.provider,
            %url,
            model = %model.model_id,
            messages = context.messages.len(),
            "sending OAI-compatible request"
        );

        let request = prepare_oai_request(&self.base.client, &url, model, context, options)
            .header("Authorization", format!("Bearer {}", self.api_key(options)));

        Box::pin(oai_send_and_parse(
            request,
            self.provider,
            cancellation_token,
            options.on_raw_payload.clone(),
            |_, _| None,
        ))
    }
}

/// Build a standard OAI-compatible HTTP request (without auth headers).
///
/// Handles message conversion, tool extraction, and body serialization.
/// Returns a `reqwest::RequestBuilder` ready for auth header injection.
///
/// Used by adapters that follow the standard OAI message format (`OpenAI`,
/// Azure, xAI). Mistral uses its own message conversion and body type.
pub fn prepare_oai_request(
    client: &reqwest::Client,
    url: &str,
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
) -> reqwest::RequestBuilder {
    let messages =
        convert::convert_messages::<OaiConverter>(&context.messages, &context.system_prompt);
    let (tools, tool_choice) = build_oai_tools(&context.tools);
    let body = OaiChatRequest {
        model: model.model_id.clone(),
        messages,
        stream: true,
        stream_options: Some(OaiStreamOptions {
            include_usage: true,
        }),
        temperature: options.temperature,
        max_tokens: options.max_tokens,
        tools,
        tool_choice,
    };
    client.post(url).json(&body)
}

/// Send an OAI-compatible request and parse the SSE response stream.
///
/// This is the shared transport pipeline for OpenAI-protocol adapters:
/// 1. Send the HTTP request
/// 2. Classify HTTP errors (provider-specific override first, then standard)
/// 3. Parse the SSE response into [`AssistantMessageEvent`]s
///
/// Callers build the `RequestBuilder` with provider-specific URL, auth,
/// and body; this function handles everything after that.
///
/// The `classify_error` callback allows providers to intercept specific
/// error conditions (e.g., Azure content filter violations). Return
/// `Some(event)` to override the default classification, `None` to fall
/// through to standard HTTP status mapping.
pub fn oai_send_and_parse<'a>(
    request: reqwest::RequestBuilder,
    provider: &'static str,
    cancellation_token: tokio_util::sync::CancellationToken,
    on_raw_payload: Option<swink_agent::OnRawPayload>,
    classify_error: impl Fn(u16, &str) -> Option<AssistantMessageEvent> + Send + 'a,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        let response = match tokio::select! {
            () = cancellation_token.cancelled() => {
                return stream::iter(Vec::from(crate::base::pre_stream_error(
                    crate::base::cancelled_error("operation cancelled"),
                )))
                .left_stream();
            }
            response = request.send() => response
        } {
            Ok(resp) => resp,
            Err(e) => {
                return stream::iter(vec![
                    AssistantMessageEvent::Start,
                    AssistantMessageEvent::error_network(format!(
                        "{provider} connection error: {e}"
                    )),
                ])
                .left_stream();
            }
        };

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status = code, "{provider} HTTP error");

            if let Some(event) = classify_error(code, &body) {
                return stream::iter(vec![AssistantMessageEvent::Start, event]).left_stream();
            }

            let event = crate::classify::error_event_from_status(code, &body, provider);
            return stream::iter(vec![AssistantMessageEvent::Start, event]).left_stream();
        }

        parse_oai_sse_stream(response, cancellation_token, provider, on_raw_payload).right_stream()
    })
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_chat_path_is_used() {
        let shell = OaiAdapterShell::new_with_path(
            "Azure",
            "https://example.openai.azure.com/openai/deployments/gpt-4/",
            "",
            "/chat/completions",
        );

        assert_eq!(
            shell.chat_completions_url(),
            "https://example.openai.azure.com/openai/deployments/gpt-4/chat/completions"
        );
    }
}
