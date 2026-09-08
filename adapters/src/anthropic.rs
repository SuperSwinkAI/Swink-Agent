//! Native Anthropic Messages API adapter.
//!
//! Implements [`StreamFn`] for the Anthropic Messages API (`/v1/messages`).
//! Handles the Anthropic-specific SSE format, including thinking blocks and
//! tool use.

use std::collections::HashMap;
use std::pin::Pin;

use futures::stream::{self, Stream, StreamExt as _};
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use swink_agent::ContentBlock;
use swink_agent::{
    AgentContext, AgentMessage, AssistantMessageEvent, CacheStrategy, Cost, LlmMessage, ModelSpec,
    ServingOptionSupport, StopReason, StreamFn, StreamOptions, ThinkingLevel, Usage,
};

use crate::base::AdapterBase;
use crate::block_accumulator::BlockAccumulator;
use crate::convert::extract_tool_schemas;
use crate::sse::{SseAction, SseEvent, sse_paired_events_with_callback};

// ─── Request types ──────────────────────────────────────────────────────────

/// A content block in an Anthropic message.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

/// Message in Anthropic's format.
#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContentBlock>,
}

/// Tool definition in Anthropic's format.
#[derive(Debug, Serialize)]
struct AnthropicToolDef {
    name: String,
    description: String,
    input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

/// System prompt content block for Anthropic.
#[derive(Debug, Serialize)]
struct SystemBlock {
    r#type: &'static str,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControl>,
}

/// Cache control marker for Anthropic.
#[derive(Debug, Clone, Serialize)]
struct CacheControl {
    r#type: &'static str,
}

/// Thinking configuration.
#[derive(Debug, Serialize)]
struct AnthropicThinking {
    r#type: String,
    budget_tokens: u64,
}

/// Full request body for Anthropic `/v1/messages`.
#[derive(Debug, Serialize)]
struct AnthropicChatRequest {
    model: String,
    max_tokens: u64,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Value>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking>,
}

// ─── SSE event / block tracking ─────────────────────────────────────────────

/// The type of content block currently active at a given provider index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockType {
    Text,
    Thinking,
    ToolUse,
}

/// State machine tracking SSE streaming progress.
///
/// Block lifecycle (index allocation, open/close, drain) is delegated to
/// [`BlockAccumulator`].  The `provider_blocks` map translates Anthropic's
/// provider-side block indices to `(BlockType, harness content_index)` so
/// that `content_block_delta` and `content_block_stop` events can be routed
/// to the correct accumulator method.
struct SseStreamState {
    /// Shared block lifecycle accumulator.
    blocks: BlockAccumulator,
    /// Anthropic block index → `(BlockType, harness content_index)`.
    provider_blocks: HashMap<usize, (BlockType, usize)>,
    usage: Usage,
    stop_reason: Option<StopReason>,
}

// NOTE: Event/data pairing is handled by `SseEvent` from `crate::sse::sse_paired_events`.

// ─── AnthropicStreamFn ──────────────────────────────────────────────────────

/// A [`StreamFn`] implementation for the Anthropic Messages API.
///
/// Connects to the Anthropic API (or a compatible endpoint) and streams
/// responses as `AssistantMessageEvent` values. Supports text, thinking,
/// and tool-use content blocks.
pub struct AnthropicStreamFn {
    base: AdapterBase,
}

impl AnthropicStreamFn {
    /// Create a new Anthropic stream function.
    ///
    /// # Arguments
    ///
    /// * `base_url` - API base URL (e.g. `https://api.anthropic.com`).
    /// * `api_key` - Anthropic API key for `x-api-key` header authentication.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base: AdapterBase::new(base_url, api_key),
        }
    }
}

impl std::fmt::Debug for AnthropicStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicStreamFn")
            .field("base_url", &self.base.base_url)
            .field("api_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl StreamFn for AnthropicStreamFn {
    // Only `extra` is merged into the request body; the typed serving
    // fields have no Anthropic-protocol equivalent.
    fn supported_serving_options(&self) -> ServingOptionSupport {
        ServingOptionSupport::none().with_extra(true)
    }

    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(anthropic_stream(
            self,
            model,
            context,
            options,
            cancellation_token,
        ))
    }
}

// ─── Stream implementation ──────────────────────────────────────────────────

fn anthropic_stream<'a>(
    anthropic: &'a AnthropicStreamFn,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        let response = match tokio::select! {
            () = cancellation_token.cancelled() => {
                return stream::iter(Vec::from(crate::base::pre_stream_error(
                    crate::base::cancelled_error("operation cancelled"),
                )))
                .left_stream();
            }
            response = send_request(anthropic, model, context, options) => response
        } {
            Ok(resp) => resp,
            Err(event) => {
                return stream::iter(Vec::from(crate::base::pre_stream_error(event))).left_stream();
            }
        };
        crate::base::report_rate_limit(response.headers(), options.on_rate_limit.as_ref());

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            // Read before the body is consumed below — Retry-After (FR-006)
            // is only present on the response headers, not the SSE/JSON body.
            let retry_after = crate::classify::parse_retry_after(response.headers());
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
            warn!(status = code, ?retry_after, "Anthropic HTTP error");
            // Anthropic-specific: context overflow arrives as HTTP 400
            // `invalid_request_error` with a documented message — classify it
            // structurally before falling back to status-based mapping.
            if let Some(event) = classify_anthropic_error_body(&body) {
                return stream::iter(Vec::from(crate::base::pre_stream_error(event))).left_stream();
            }
            // Anthropic-specific: 529 (overloaded) and 504 (gateway timeout)
            // are retryable network errors.
            let event = crate::classify::error_event_from_status_with_overrides(
                code,
                &body,
                "Anthropic",
                &[
                    (529, crate::classify::HttpErrorKind::Network),
                    (504, crate::classify::HttpErrorKind::Network),
                ],
            );
            let event = crate::classify::with_retry_after(event, retry_after);
            return stream::iter(Vec::from(crate::base::pre_stream_error(event))).left_stream();
        }

        parse_sse_stream(response, cancellation_token, options.on_raw_payload.clone())
            .right_stream()
    })
    .flatten()
}

/// Classify an Anthropic HTTP error body by its structured `error.type`.
///
/// Anthropic reports context-window overflow as HTTP 400
/// `invalid_request_error` with a message like
/// `"prompt is too long: 210510 tokens > 200000 maximum"` — there is no
/// dedicated error type, so the adapter matches the documented wording
/// (scoped to `invalid_request_error`) and attaches the structured
/// `StreamErrorKind::ContextWindowExceeded`.
///
/// Returns `None` when the body doesn't identify a more specific condition,
/// so the caller falls through to HTTP-status classification.
fn classify_anthropic_error_body(body: &str) -> Option<AssistantMessageEvent> {
    let value: Value = serde_json::from_str(body).ok()?;
    let error_type = value.pointer("/error/type").and_then(Value::as_str)?;
    let message = value.pointer("/error/message").and_then(Value::as_str)?;
    (error_type == "invalid_request_error" && crate::classify::is_context_overflow_message(message))
        .then(|| {
            AssistantMessageEvent::error_context_overflow(format!(
                "Anthropic context window exceeded: {message}"
            ))
        })
}

/// Send the HTTP POST request to the Anthropic Messages API.
async fn send_request(
    anthropic: &AnthropicStreamFn,
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
) -> Result<reqwest::Response, AssistantMessageEvent> {
    let url = format!("{}/v1/messages", anthropic.base.base_url);
    debug!(
        %url,
        model = %model.model_id,
        messages = context.messages.len(),
        "sending Anthropic request"
    );

    let (system_text, messages) = convert_messages(&context.messages, &context.system_prompt);

    let use_caching = matches!(
        options.cache_strategy,
        CacheStrategy::Auto | CacheStrategy::Anthropic
    );

    let mut tools: Vec<AnthropicToolDef> = extract_tool_schemas(&context.tools)
        .into_iter()
        .map(|s| AnthropicToolDef {
            name: s.name,
            description: s.description,
            input_schema: s.parameters,
            cache_control: None,
        })
        .collect();

    // Apply cache strategy: inject cache_control on system prompt and last tool def
    let system = if use_caching {
        if let Some(last) = tools.last_mut() {
            last.cache_control = Some(CacheControl {
                r#type: "ephemeral",
            });
        }
        system_text.map(|text| {
            serde_json::to_value(vec![SystemBlock {
                r#type: "text",
                text,
                cache_control: Some(CacheControl {
                    r#type: "ephemeral",
                }),
            }])
            .unwrap_or(Value::Null)
        })
    } else {
        system_text.map(Value::String)
    };

    let max_tokens = options.max_tokens.unwrap_or(4096);

    // Resolve thinking budget from model spec
    let thinking = resolve_thinking(model, max_tokens);

    // When thinking is enabled, temperature must not be set (Anthropic requires
    // temperature=1 which is the default when omitted).
    let temperature = if thinking.is_some() {
        None
    } else {
        options.temperature
    };

    let body = AnthropicChatRequest {
        model: model.model_id.clone(),
        max_tokens,
        stream: true,
        system,
        messages,
        tools,
        temperature,
        thinking,
    };

    let api_key = options
        .api_key
        .as_deref()
        .unwrap_or(&anthropic.base.api_key);

    let request = anthropic
        .base
        .client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json");

    // `ServingOptions::extra` merges into the top level of the `/v1/messages`
    // body — that is where Anthropic's provider-native knobs live (`top_k`,
    // `stop_sequences`, `metadata`, `service_tier`, …). Typed fields win on
    // collision; a key the API doesn't know is rejected loudly with HTTP 400
    // rather than silently dropped. The empty-`extra` fast path serializes the
    // typed struct directly, keeping default request bytes untouched.
    let request = if options.serving.extra.is_empty() {
        request.json(&body)
    } else {
        const TYPED_KEYS: &[&str] = &[
            "model",
            "max_tokens",
            "stream",
            "system",
            "messages",
            "tools",
            "temperature",
            "thinking",
        ];
        let mut value = serde_json::to_value(&body).map_err(|e| {
            AssistantMessageEvent::error_network(format!("Anthropic JSON error: {e}"))
        })?;
        if let Value::Object(map) = &mut value {
            crate::base::merge_extra(map, &options.serving.extra, TYPED_KEYS);
        }
        request.json(&value)
    };

    request.send().await.map_err(|e| {
        AssistantMessageEvent::error_network(format!("Anthropic connection error: {e}"))
    })
}

/// Resolve thinking configuration from the model spec.
fn resolve_thinking(model: &ModelSpec, max_tokens: u64) -> Option<AnthropicThinking> {
    if model.thinking_level == ThinkingLevel::Off {
        return None;
    }

    // Try to get a budget from the thinking_budgets map first, then use defaults.
    let budget = model
        .thinking_budgets
        .as_ref()
        .and_then(|b| b.get(&model.thinking_level))
        .unwrap_or_else(|| match model.thinking_level {
            ThinkingLevel::Minimal => 1024,
            ThinkingLevel::Low => 2048,
            ThinkingLevel::High => 10_000,
            ThinkingLevel::ExtraHigh => 20_000,
            ThinkingLevel::Off => unreachable!(),
            // Covers ThinkingLevel::Medium and, since ThinkingLevel is
            // #[non_exhaustive], any future variant not yet known to this
            // adapter — both fall back to the Medium-tier budget.
            _ => 5000,
        });

    // Anthropic requires `budget_tokens` to be strictly less than `max_tokens`.
    // Silently capping here is intentional — callers set budgets in terms of the
    // thinking level, not the absolute token limit, so exceeding max_tokens is a
    // normal edge case rather than a user error worth surfacing.
    let budget = budget.min(max_tokens.saturating_sub(1));

    Some(AnthropicThinking {
        r#type: "enabled".to_string(),
        budget_tokens: budget,
    })
}

/// Convert harness messages to Anthropic message format.
///
/// This function uses a bespoke conversion instead of the shared
/// [`MessageConverter`](super::convert::MessageConverter) trait because
/// the Anthropic API requires the system prompt as a separate top-level
/// field rather than as a message, and thinking blocks must be filtered
/// from outgoing requests.
///
/// Returns `(system, messages)` — the system prompt is a top-level field in
/// Anthropic's API, not a message.
fn convert_messages(
    messages: &[AgentMessage],
    system_prompt: &str,
) -> (Option<String>, Vec<AnthropicMessage>) {
    let system = if system_prompt.is_empty() {
        None
    } else {
        Some(system_prompt.to_string())
    };

    let mut result: Vec<AnthropicMessage> = Vec::new();

    for msg in messages {
        let AgentMessage::Llm(llm) = msg else {
            continue;
        };
        match llm {
            LlmMessage::User(user) => {
                let text = ContentBlock::extract_text(&user.content);
                result.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: vec![AnthropicContentBlock::Text { text }],
                });
            }
            LlmMessage::Assistant(assistant) => {
                let mut content = Vec::new();
                for block in &assistant.content {
                    match block {
                        ContentBlock::Text { text } if !text.is_empty() => {
                            content.push(AnthropicContentBlock::Text { text: text.clone() });
                        }
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => {
                            // Issue #619: loop-level scrub coerces incomplete tool-use
                            // blocks into object-typed arguments before reaching here.
                            // Debug builds assert the invariant to catch regressions.
                            debug_assert!(
                                arguments.is_object(),
                                "anthropic adapter: tool_use arguments must be a JSON object (got {arguments:?}); loop-level sanitize_incomplete_tool_calls should have coerced this before dispatch"
                            );
                            content.push(AnthropicContentBlock::ToolUse {
                                id: id.clone(),
                                name: name.clone(),
                                input: arguments.clone(),
                            });
                        }
                        // Skip thinking and other blocks — Anthropic doesn't accept them back.
                        _ => {}
                    }
                }
                if !content.is_empty() {
                    result.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content,
                    });
                }
            }
            LlmMessage::ToolResult(tool_result) => {
                let text = ContentBlock::extract_text(&tool_result.content);
                let block = AnthropicContentBlock::ToolResult {
                    tool_use_id: tool_result.tool_call_id.clone(),
                    content: text,
                };

                // Combine consecutive tool results into a single user message.
                if let Some(last) = result.last_mut()
                    && last.role == "user"
                    && last
                        .content
                        .iter()
                        .all(|b| matches!(b, AnthropicContentBlock::ToolResult { .. }))
                {
                    last.content.push(block);
                    continue;
                }

                result.push(AnthropicMessage {
                    role: "user".to_string(),
                    content: vec![block],
                });
            }
            // Unknown future LlmMessage variant: nothing sensible to send to
            // Anthropic, so drop it — same as messages skipped elsewhere in
            // this loop (e.g. non-LLM AgentMessage variants).
            &_ => {}
        }
    }

    (system, result)
}

/// Parse Anthropic's SSE streaming response into `AssistantMessageEvent` values.
#[allow(clippy::too_many_lines)]
fn parse_sse_stream(
    response: reqwest::Response,
    cancellation_token: CancellationToken,
    on_raw_payload: Option<swink_agent::OnRawPayload>,
) -> impl Stream<Item = AssistantMessageEvent> + Send {
    let line_stream = sse_paired_events_with_callback(response.bytes_stream(), on_raw_payload);

    let state = SseStreamState {
        blocks: BlockAccumulator::default(),
        provider_blocks: HashMap::new(),
        usage: Usage::default(),
        stop_reason: None,
    };

    crate::sse::sse_adapter_stream(
        line_stream,
        cancellation_token,
        state,
        "operation cancelled",
        |item, state| match item {
            None => {
                let mut events = crate::finalize::finalize_blocks(state);
                events.push(AssistantMessageEvent::error_network(
                    "Anthropic stream ended unexpectedly",
                ));
                SseAction::Done(events)
            }
            Some(SseEvent { event_type, data })
                if event_type == crate::sse::SSE_TRANSPORT_ERROR_EVENT =>
            {
                let mut events = crate::finalize::finalize_blocks(state);
                events.push(AssistantMessageEvent::error_network(format!(
                    "Anthropic {data}",
                )));
                SseAction::Done(events)
            }
            Some(SseEvent { event_type, data })
                if event_type == crate::sse::SSE_PROTOCOL_ERROR_EVENT =>
            {
                let mut events = crate::finalize::finalize_blocks(state);
                events.push(AssistantMessageEvent::error(format!("Anthropic {data}")));
                SseAction::Done(events)
            }
            Some(SseEvent { event_type, data }) => {
                let mut done = false;
                let events = process_sse_event(&event_type, &data, state, &mut done);
                if done {
                    SseAction::Done(events)
                } else {
                    SseAction::Continue(events)
                }
            }
        },
    )
}

fn malformed_event_parse_error(
    state: &mut SseStreamState,
    event_type: &str,
    error: &serde_json::Error,
) -> Vec<AssistantMessageEvent> {
    error!(event_type, error = %error, "Anthropic SSE JSON parse error");
    let mut events = crate::finalize::finalize_blocks(state);
    events.push(AssistantMessageEvent::error(format!(
        "Anthropic {event_type} JSON parse error: {error}",
    )));
    events
}

fn malformed_event_protocol_error(
    state: &mut SseStreamState,
    event_type: &str,
    message: impl std::fmt::Display,
) -> Vec<AssistantMessageEvent> {
    error!(event_type, error = %message, "Anthropic SSE protocol error");
    let mut events = crate::finalize::finalize_blocks(state);
    events.push(AssistantMessageEvent::error(format!(
        "Anthropic {event_type} protocol error: {message}",
    )));
    events
}

fn required_index(parsed: &Value) -> Result<usize, String> {
    let Some(index) = parsed.get("index") else {
        return Err("missing required field `index`".to_string());
    };
    let Some(index) = index.as_u64() else {
        return Err("required field `index` must be a non-negative integer".to_string());
    };
    index
        .try_into()
        .map_err(|_| "required field `index` exceeds platform usize".to_string())
}

fn required_non_empty_str<'a>(parsed: &'a Value, pointer: &str) -> Result<&'a str, String> {
    let Some(value) = parsed.pointer(pointer).and_then(Value::as_str) else {
        return Err(format!("missing required field `{pointer}`"));
    };
    if value.is_empty() {
        return Err(format!("required field `{pointer}` must not be empty"));
    }
    Ok(value)
}

/// Process a single SSE event and return the resulting harness events.
#[allow(clippy::too_many_lines)]
fn process_sse_event(
    event_type: &str,
    data: &str,
    state: &mut SseStreamState,
    done: &mut bool,
) -> Vec<AssistantMessageEvent> {
    let mut events = Vec::new();

    match event_type {
        "message_start" => {
            // Extract input token usage from message_start
            let parsed = match serde_json::from_str::<Value>(data) {
                Ok(parsed) => parsed,
                Err(parse_error) => {
                    *done = true;
                    return malformed_event_parse_error(state, event_type, &parse_error);
                }
            };
            if let Some(input) = parsed
                .pointer("/message/usage/input_tokens")
                .and_then(Value::as_u64)
            {
                state.usage.input = input;
            }
            if let Some(cache_read) = parsed
                .pointer("/message/usage/cache_read_input_tokens")
                .and_then(Value::as_u64)
            {
                state.usage.cache_read = cache_read;
            }
            if let Some(cache_write) = parsed
                .pointer("/message/usage/cache_creation_input_tokens")
                .and_then(Value::as_u64)
            {
                state.usage.cache_write = cache_write;
            }
        }

        "content_block_start" => {
            let parsed = match serde_json::from_str::<Value>(data) {
                Ok(parsed) => parsed,
                Err(parse_error) => {
                    *done = true;
                    return malformed_event_parse_error(state, event_type, &parse_error);
                }
            };
            let index = match required_index(&parsed) {
                Ok(index) => index,
                Err(error) => {
                    *done = true;
                    return malformed_event_protocol_error(state, event_type, error);
                }
            };
            let block_type = parsed
                .pointer("/content_block/type")
                .and_then(Value::as_str)
                .unwrap_or("");

            match block_type {
                "text" => {
                    events.extend(state.blocks.ensure_text_open());
                    // Always register the provider→harness index mapping so
                    // subsequent content_block_delta events can route by
                    // provider index.  ensure_text_open is idempotent, but
                    // the provider may use a fresh index for the same block.
                    if let Some(content_index) = state.blocks.text_index() {
                        state
                            .provider_blocks
                            .insert(index, (BlockType::Text, content_index));
                    }
                }
                "thinking" => {
                    events.extend(state.blocks.ensure_thinking_open());
                    if let Some(content_index) = state.blocks.thinking_index() {
                        state
                            .provider_blocks
                            .insert(index, (BlockType::Thinking, content_index));
                    }
                }
                "tool_use" => {
                    let id = match required_non_empty_str(&parsed, "/content_block/id") {
                        Ok(id) => id.to_string(),
                        Err(error) => {
                            *done = true;
                            return malformed_event_protocol_error(state, event_type, error);
                        }
                    };
                    let name = match required_non_empty_str(&parsed, "/content_block/name") {
                        Ok(name) => name.to_string(),
                        Err(error) => {
                            *done = true;
                            return malformed_event_protocol_error(state, event_type, error);
                        }
                    };
                    let (content_index, start_ev) = state.blocks.open_tool_call(id, name);
                    state
                        .provider_blocks
                        .insert(index, (BlockType::ToolUse, content_index));
                    events.push(start_ev);
                }
                _ => {}
            }
        }

        "content_block_delta" => {
            let parsed = match serde_json::from_str::<Value>(data) {
                Ok(parsed) => parsed,
                Err(parse_error) => {
                    *done = true;
                    return malformed_event_parse_error(state, event_type, &parse_error);
                }
            };
            let index = match required_index(&parsed) {
                Ok(index) => index,
                Err(error) => {
                    *done = true;
                    return malformed_event_protocol_error(state, event_type, error);
                }
            };
            let delta_type = parsed
                .pointer("/delta/type")
                .and_then(Value::as_str)
                .unwrap_or("");

            if let Some(&(block_type, content_index)) = state.provider_blocks.get(&index) {
                match delta_type {
                    "text_delta" => {
                        debug_assert!(
                            matches!(block_type, BlockType::Text),
                            "text_delta on non-text provider block"
                        );
                        if let Some(text) = parsed.pointer("/delta/text").and_then(Value::as_str) {
                            // Use the provider-mapped content_index so the
                            // event always carries the index registered at
                            // content_block_start — matching pre-migration
                            // behaviour exactly.
                            events.push(AssistantMessageEvent::TextDelta {
                                content_index,
                                delta: text.to_string(),
                            });
                        }
                    }
                    "thinking_delta" => {
                        debug_assert!(
                            matches!(block_type, BlockType::Thinking),
                            "thinking_delta on non-thinking provider block"
                        );
                        if let Some(thinking) =
                            parsed.pointer("/delta/thinking").and_then(Value::as_str)
                        {
                            events.push(AssistantMessageEvent::ThinkingDelta {
                                content_index,
                                delta: thinking.to_string(),
                            });
                        }
                    }
                    "signature_delta" => {
                        debug_assert!(
                            matches!(block_type, BlockType::Thinking),
                            "signature_delta on non-thinking provider block"
                        );
                        if matches!(block_type, BlockType::Thinking)
                            && let Some(signature) =
                                parsed.pointer("/delta/signature").and_then(Value::as_str)
                        {
                            state.blocks.set_thinking_signature(signature.to_string());
                        }
                    }
                    "input_json_delta" => {
                        if let Some(json) = parsed
                            .pointer("/delta/partial_json")
                            .and_then(Value::as_str)
                        {
                            events.push(BlockAccumulator::tool_call_delta(
                                content_index,
                                json.to_string(),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }

        "content_block_stop" => {
            let parsed = match serde_json::from_str::<Value>(data) {
                Ok(parsed) => parsed,
                Err(parse_error) => {
                    *done = true;
                    return malformed_event_parse_error(state, event_type, &parse_error);
                }
            };
            let index = match required_index(&parsed) {
                Ok(index) => index,
                Err(error) => {
                    *done = true;
                    return malformed_event_protocol_error(state, event_type, error);
                }
            };

            if let Some((block_type, content_index)) = state.provider_blocks.remove(&index) {
                match block_type {
                    BlockType::Text => {
                        events.extend(state.blocks.close_text());
                    }
                    BlockType::Thinking => {
                        let signature = parsed
                            .pointer("/signature")
                            .and_then(Value::as_str)
                            .map(String::from);
                        events.extend(state.blocks.close_thinking(signature));
                    }
                    BlockType::ToolUse => {
                        events.extend(state.blocks.close_tool_call(content_index));
                    }
                }
            }
        }

        "message_delta" => {
            let parsed = match serde_json::from_str::<Value>(data) {
                Ok(parsed) => parsed,
                Err(parse_error) => {
                    *done = true;
                    return malformed_event_parse_error(state, event_type, &parse_error);
                }
            };
            // Extract stop reason
            if let Some(reason) = parsed.pointer("/delta/stop_reason").and_then(Value::as_str) {
                state.stop_reason = Some(match reason {
                    "tool_use" => StopReason::ToolUse,
                    "max_tokens" => StopReason::Length,
                    _ => StopReason::Stop,
                });
            }

            // Extract output token usage
            if let Some(output) = parsed
                .pointer("/usage/output_tokens")
                .and_then(Value::as_u64)
            {
                state.usage.output = output;
            }
        }

        "message_stop" => {
            *done = true;
            events.extend(crate::finalize::finalize_blocks(state));

            let stop_reason = state.stop_reason.unwrap_or(StopReason::Stop);
            state.usage.total = state.usage.input
                + state.usage.output
                + state.usage.cache_read
                + state.usage.cache_write;

            events.push(AssistantMessageEvent::Done {
                stop_reason,
                usage: state.usage.clone(),
                cost: Cost::default(),
            });
        }

        "error" => {
            *done = true;
            let parsed = match serde_json::from_str::<Value>(data) {
                Ok(parsed) => Some(parsed),
                Err(parse_error) => {
                    return malformed_event_parse_error(state, event_type, &parse_error);
                }
            };
            events.extend(crate::finalize::finalize_blocks(state));
            let msg = parsed
                .as_ref()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(String::from)
                })
                .unwrap_or_else(|| format!("Anthropic stream error: {data}"));
            let error_type = parsed
                .as_ref()
                .and_then(|v| v.pointer("/error/type").and_then(Value::as_str));

            error!(error = %msg, "Anthropic stream error");

            let event = match error_type {
                Some("authentication_error" | "permission_error") => {
                    AssistantMessageEvent::error_auth(&msg)
                }
                Some("rate_limit_error") => AssistantMessageEvent::error_throttled(&msg),
                Some("invalid_request_error") => {
                    if crate::classify::is_context_overflow_message(&msg) {
                        // "prompt is too long: X tokens > Y maximum"
                        AssistantMessageEvent::error_context_overflow(&msg)
                    } else {
                        // Deterministic client error — not a retryable
                        // network fault.
                        AssistantMessageEvent::error(&msg)
                    }
                }
                Some("overloaded_error" | "api_error" | "server_error") => {
                    AssistantMessageEvent::error_network(&msg)
                }
                _ => AssistantMessageEvent::error(&msg),
            };
            events.push(event);
        }

        // Ignore ping and other unknown event types
        _ => {}
    }

    events
}

impl crate::finalize::StreamFinalize for SseStreamState {
    fn drain_open_blocks(&mut self) -> Vec<crate::finalize::OpenBlock> {
        self.provider_blocks.clear();
        crate::finalize::StreamFinalize::drain_open_blocks(&mut self.blocks)
    }
}

// Event/data pairing is now handled by `crate::sse::sse_paired_events`.

// ─── Compile-time assertions ────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AnthropicStreamFn>();
};

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
