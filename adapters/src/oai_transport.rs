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

use swink_agent::{AgentContext, AssistantMessageEvent, ModelSpec, ResponseFormat, StreamOptions};

use crate::base::AdapterBase;
use crate::convert;
use crate::openai_compat::{
    OaiChatRequest, OaiConverter, OaiParserOptions, OaiStreamOptions, build_oai_tools,
    parse_oai_sse_stream_with_options,
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
    // Callers are the `openai`, `xai` and `mistral` adapters. `openai-compat`
    // is an internal umbrella that `openai`/`xai` both imply, so gating on it
    // was too broad: enabling `openai-compat` on its own left this dead.
    #[cfg(any(feature = "openai", feature = "xai", feature = "mistral"))]
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

        let provider = self.provider;
        Box::pin(oai_send_and_parse(
            request,
            provider,
            cancellation_token,
            options.on_raw_payload.clone(),
            move |status, body| classify_oai_error_body(status, body, provider),
        ))
    }
}

/// Classify an OpenAI-compatible HTTP 4xx error body into a structured error
/// event when the payload carries a recognizable provider code or message.
///
/// Handles three envelope shapes:
/// - standard OAI: `{"error": {"message", "type", "code"}}`
///   (`OpenAI`, Azure) — `code: "context_length_exceeded"` and
///   `code: "content_filter"` are structured signals;
/// - Mistral top-level: `{"object": "error", "message", "type", "code"}`;
/// - xAI string form: `{"code": "...", "error": "message text"}`.
///
/// Providers without a structured code for context overflow are matched
/// against their documented message wording via
/// [`classify::is_context_overflow_message`](crate::classify::is_context_overflow_message).
///
/// Returns `None` when the body doesn't identify a more specific condition,
/// so callers fall through to standard HTTP-status classification.
pub(crate) fn classify_oai_error_body(
    status: u16,
    body: &str,
    provider: &str,
) -> Option<AssistantMessageEvent> {
    if !(400..500).contains(&status) {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let (code, message) = match value.get("error") {
        // xAI: {"code": "...", "error": "message text"}
        Some(serde_json::Value::String(message)) => (None, Some(message.as_str())),
        // Standard OAI: {"error": {"message", "type", "code"}}
        Some(error) => (
            error.get("code").and_then(serde_json::Value::as_str),
            error.get("message").and_then(serde_json::Value::as_str),
        ),
        // Mistral: {"object": "error", "message": "...", "code": ...}
        None => (
            value.get("code").and_then(serde_json::Value::as_str),
            value.get("message").and_then(serde_json::Value::as_str),
        ),
    };
    let message = message.unwrap_or(body);

    if code == Some("context_length_exceeded")
        || crate::classify::is_context_overflow_message(message)
    {
        return Some(AssistantMessageEvent::error_context_overflow(format!(
            "{provider} context window exceeded (HTTP {status}): {message}"
        )));
    }
    if code == Some("content_filter") {
        return Some(AssistantMessageEvent::error_content_filtered(format!(
            "{provider} content filter (HTTP {status}): {message}"
        )));
    }
    None
}

/// Map [`ResponseFormat`] onto the OAI protocol's `response_format` field.
///
/// `Json` becomes `{"type": "json_object"}`. `Schema` is a bare JSON Schema
/// (what Ollama takes verbatim), so it gets wrapped in the `json_schema`
/// envelope this protocol requires; `strict` opts into constrained decoding.
///
/// `allow(dead_code)`: reachable only from [`prepare_oai_request`], which is
/// itself live only under a consuming adapter feature.
#[allow(dead_code)]
fn oai_response_format(options: &StreamOptions) -> Option<serde_json::Value> {
    options
        .serving
        .format
        .as_ref()
        .and_then(|format| match format {
            ResponseFormat::Json => Some(serde_json::json!({ "type": "json_object" })),
            ResponseFormat::Schema(schema) => Some(serde_json::json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "response",
                    "strict": true,
                    "schema": schema,
                },
            })),
            // Unknown future variant: we don't know how to represent it on the
            // wire, so omit `response_format` entirely rather than sending
            // something wrong.
            _ => None,
        })
}

/// Build a standard OAI-compatible HTTP request (without auth headers).
///
/// Handles message conversion, tool extraction, and body serialization.
/// Returns a `reqwest::RequestBuilder` ready for auth header injection.
///
/// Used by adapters that follow the standard OAI message format (`OpenAI`,
/// Azure, xAI). Mistral uses its own message conversion and body type.
///
/// `allow(dead_code)`: live only when a consuming adapter feature is enabled,
/// so it is dead under feature combinations that pull in none of them (same
/// rationale as [`oai_send_and_parse`] below). This also keeps the
/// `OaiChatRequest`/`OaiStreamOptions` bodies it constructs reachable.
#[allow(dead_code)]
pub fn prepare_oai_request(
    client: &reqwest::Client,
    url: &str,
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
) -> reqwest::RequestBuilder {
    // Typed request fields win over colliding `ServingOptions::extra` keys.
    // (`context_length`/`keep_alive` have no OAI-protocol equivalent and are
    // intentionally not serialized here — see the `ServingOptions` docs.)
    const TYPED_KEYS: &[&str] = &[
        "model",
        "messages",
        "stream",
        "stream_options",
        "temperature",
        "max_tokens",
        "top_p",
        "response_format",
        "tools",
        "tool_choice",
    ];
    let mut extra = serde_json::Map::new();
    crate::base::merge_extra(&mut extra, &options.serving.extra, TYPED_KEYS);

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
        top_p: options.serving.top_p,
        response_format: oai_response_format(options),
        tools,
        tool_choice,
        extra,
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
#[allow(dead_code)]
pub fn oai_send_and_parse<'a>(
    request: reqwest::RequestBuilder,
    provider: &'static str,
    cancellation_token: tokio_util::sync::CancellationToken,
    on_raw_payload: Option<swink_agent::OnRawPayload>,
    classify_error: impl Fn(u16, &str) -> Option<AssistantMessageEvent> + Send + 'a,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    oai_send_and_parse_with_options(
        request,
        provider,
        cancellation_token,
        on_raw_payload,
        classify_error,
        OaiParserOptions::default(),
    )
}

pub(crate) fn oai_send_and_parse_with_options<'a>(
    request: reqwest::RequestBuilder,
    provider: &'static str,
    cancellation_token: tokio_util::sync::CancellationToken,
    on_raw_payload: Option<swink_agent::OnRawPayload>,
    classify_error: impl Fn(u16, &str) -> Option<AssistantMessageEvent> + Send + 'a,
    parser_options: OaiParserOptions,
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
            let body = match crate::base::read_error_body_or_cancelled(
                response,
                &cancellation_token,
                "operation cancelled",
            )
            .await
            {
                Ok(body) => body,
                Err(event) => {
                    return stream::iter(Vec::from(crate::base::pre_stream_error(event)))
                        .left_stream();
                }
            };
            warn!(status = code, "{provider} HTTP error");

            if let Some(event) = classify_error(code, &body) {
                return stream::iter(vec![AssistantMessageEvent::Start, event]).left_stream();
            }

            let event = crate::classify::error_event_from_status(code, &body, provider);
            return stream::iter(vec![AssistantMessageEvent::Start, event]).left_stream();
        }

        parse_oai_sse_stream_with_options(
            response,
            cancellation_token,
            provider,
            on_raw_payload,
            parser_options,
        )
        .right_stream()
    })
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oai_body_context_length_exceeded_code_is_context_overflow() {
        let body = r#"{"error":{"message":"This model's maximum context length is 128000 tokens. However, your messages resulted in 131000 tokens.","type":"invalid_request_error","param":"messages","code":"context_length_exceeded"}}"#;
        let event = classify_oai_error_body(400, body, "OpenAI").expect("expected classification");
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(
                    error_kind,
                    Some(swink_agent::StreamErrorKind::ContextWindowExceeded)
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn mistral_top_level_body_too_large_is_context_overflow() {
        let body = r#"{"object":"error","message":"Prompt contains 40960 tokens, too large for model with 32768 maximum context length","type":"invalid_request_error","param":null,"code":null}"#;
        let event = classify_oai_error_body(400, body, "Mistral").expect("expected classification");
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(
                    error_kind,
                    Some(swink_agent::StreamErrorKind::ContextWindowExceeded)
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn xai_string_error_body_prompt_length_is_context_overflow() {
        let body = r#"{"code":"Client specified an invalid argument","error":"This model's maximum prompt length is 131072 but the request contains 200000 tokens."}"#;
        let event = classify_oai_error_body(400, body, "xAI").expect("expected classification");
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(
                    error_kind,
                    Some(swink_agent::StreamErrorKind::ContextWindowExceeded)
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn oai_body_content_filter_code_is_content_filtered() {
        let body = r#"{"error":{"message":"The response was filtered due to the prompt triggering content management policy.","type":null,"code":"content_filter"}}"#;
        let event = classify_oai_error_body(400, body, "Azure").expect("expected classification");
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(
                    error_kind,
                    Some(swink_agent::StreamErrorKind::ContentFiltered)
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn oai_body_classification_only_applies_to_4xx() {
        let body = r#"{"error":{"message":"maximum context length exceeded","code":"context_length_exceeded"}}"#;
        assert!(classify_oai_error_body(500, body, "OpenAI").is_none());
    }

    #[test]
    fn unrecognized_oai_body_falls_through() {
        assert!(classify_oai_error_body(400, "not json", "OpenAI").is_none());
        assert!(
            classify_oai_error_body(
                400,
                r#"{"error":{"message":"invalid api key","code":"invalid_api_key"}}"#,
                "OpenAI"
            )
            .is_none()
        );
    }

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
