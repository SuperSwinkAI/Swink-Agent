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

use futures::stream::{self, Stream, StreamExt as _};
use tracing::warn;

use swink_agent::stream::{AssistantMessageEvent, StreamOptions};
use swink_agent::types::{AgentContext, ModelSpec};

use crate::convert;
use crate::openai_compat::{
    OaiChatRequest, OaiConverter, OaiStreamOptions, build_oai_tools, parse_oai_sse_stream,
};

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
    classify_error: impl Fn(u16, &str) -> Option<AssistantMessageEvent> + Send + 'a,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        let response = match request.send().await {
            Ok(resp) => resp,
            Err(e) => {
                return stream::iter(vec![AssistantMessageEvent::error_network(format!(
                    "{provider} connection error: {e}"
                ))])
                .left_stream();
            }
        };

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status = code, "{provider} HTTP error");

            if let Some(event) = classify_error(code, &body) {
                return stream::iter(vec![event]).left_stream();
            }

            let event = crate::classify::error_event_from_status(code, &body, provider);
            return stream::iter(vec![event]).left_stream();
        }

        parse_oai_sse_stream(response, cancellation_token, provider).right_stream()
    })
    .flatten()
}
