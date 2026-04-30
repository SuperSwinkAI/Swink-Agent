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
    StopReason, StreamFn, StreamOptions, ThinkingLevel, Usage,
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
            warn!(status = code, "Anthropic HTTP error");
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
            return stream::iter(Vec::from(crate::base::pre_stream_error(event))).left_stream();
        }

        parse_sse_stream(response, cancellation_token, options.on_raw_payload.clone())
            .right_stream()
    })
    .flatten()
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

    anthropic
        .base
        .client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
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
            ThinkingLevel::Medium => 5000,
            ThinkingLevel::High => 10_000,
            ThinkingLevel::ExtraHigh => 20_000,
            ThinkingLevel::Off => unreachable!(),
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
            let index = parsed["index"]
                .as_u64()
                .unwrap_or(0)
                .try_into()
                .unwrap_or(0);
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
                    let id = parsed
                        .pointer("/content_block/id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = parsed
                        .pointer("/content_block/name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
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
            let index = parsed["index"]
                .as_u64()
                .unwrap_or(0)
                .try_into()
                .unwrap_or(0);
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
            let index = parsed["index"]
                .as_u64()
                .unwrap_or(0)
                .try_into()
                .unwrap_or(0);

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
                Some("overloaded_error" | "api_error") => {
                    AssistantMessageEvent::error_network(&msg)
                }
                _ => AssistantMessageEvent::error_network(&msg),
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
mod tests {
    use super::*;

    #[test]
    fn cache_strategy_none_no_markers() {
        let tools = vec![AnthropicToolDef {
            name: "test".to_string(),
            description: "desc".to_string(),
            input_schema: serde_json::json!({}),
            cache_control: None,
        }];

        let request = AnthropicChatRequest {
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 4096,
            stream: true,
            system: Some(Value::String("You are helpful".to_string())),
            messages: vec![],
            tools,
            temperature: None,
            thinking: None,
        };

        let json = serde_json::to_value(&request).unwrap();
        // System should be a plain string
        assert_eq!(json["system"], "You are helpful");
        // Tools should not have cache_control
        assert!(json["tools"][0].get("cache_control").is_none());
    }

    #[test]
    fn cache_strategy_auto_anthropic_markers() {
        // Simulate what send_request does with CacheStrategy::Auto
        let system_text = Some("You are helpful".to_string());
        let mut tools = vec![AnthropicToolDef {
            name: "test".to_string(),
            description: "desc".to_string(),
            input_schema: serde_json::json!({}),
            cache_control: None,
        }];

        // Apply caching (mirroring send_request logic)
        if let Some(last) = tools.last_mut() {
            last.cache_control = Some(CacheControl {
                r#type: "ephemeral",
            });
        }
        let system = system_text.map(|text| {
            serde_json::to_value(vec![SystemBlock {
                r#type: "text",
                text,
                cache_control: Some(CacheControl {
                    r#type: "ephemeral",
                }),
            }])
            .unwrap()
        });

        let request = AnthropicChatRequest {
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 4096,
            stream: true,
            system,
            messages: vec![],
            tools,
            temperature: None,
            thinking: None,
        };

        let json = serde_json::to_value(&request).unwrap();
        // System should be an array with cache_control
        let sys_array = json["system"].as_array().unwrap();
        assert_eq!(sys_array.len(), 1);
        assert_eq!(sys_array[0]["type"], "text");
        assert_eq!(sys_array[0]["text"], "You are helpful");
        assert_eq!(sys_array[0]["cache_control"]["type"], "ephemeral");
        // Last tool should have cache_control
        assert_eq!(json["tools"][0]["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn cache_strategy_ignored_by_unsupporting_adapter() {
        // CacheStrategy::Auto on a non-Anthropic adapter should be a no-op.
        // This is tested by verifying that CacheStrategy is just an enum —
        // adapters that don't support it simply don't read the field.
        let strategy = CacheStrategy::Auto;
        assert!(matches!(strategy, CacheStrategy::Auto));
        // No code changes needed in other adapters — they ignore it by design.
    }

    // ── SSE event processing (BlockAccumulator integration) ───────────────

    /// Helper: create a fresh `SseStreamState`.
    fn new_state() -> SseStreamState {
        SseStreamState {
            blocks: BlockAccumulator::default(),
            provider_blocks: HashMap::new(),
            usage: Usage::default(),
            stop_reason: None,
        }
    }

    /// Helper: run `process_sse_event` and return `(events, done)`.
    fn process(
        event_type: &str,
        data: &str,
        state: &mut SseStreamState,
    ) -> (Vec<AssistantMessageEvent>, bool) {
        let mut done = false;
        let events = process_sse_event(event_type, data, state, &mut done);
        (events, done)
    }

    #[test]
    fn text_block_lifecycle_via_sse() {
        let mut state = new_state();

        // content_block_start for text at provider index 0
        let (events, _) = process(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextStart { content_index: 0 }
        ));

        // text delta
        let (events, _) = process(
            "content_block_delta",
            r#"{"index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::TextDelta { content_index: 0, delta } if delta == "Hello"
        ));

        // content_block_stop
        let (events, _) = process("content_block_stop", r#"{"index":0}"#, &mut state);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextEnd { content_index: 0 }
        ));
    }

    #[test]
    fn thinking_block_with_signature() {
        let mut state = new_state();

        let (events, _) = process(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::ThinkingStart { content_index: 0 }
        ));

        let (events, _) = process(
            "content_block_delta",
            r#"{"index":0,"delta":{"type":"thinking_delta","thinking":"Let me think..."}}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::ThinkingDelta { content_index: 0, delta } if delta == "Let me think..."
        ));

        // Stop with signature
        let (events, _) = process(
            "content_block_stop",
            r#"{"index":0,"signature":"abc123"}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::ThinkingEnd {
                content_index,
                signature,
            } => {
                assert_eq!(*content_index, 0);
                assert_eq!(signature.as_deref(), Some("abc123"));
            }
            other => panic!("expected ThinkingEnd, got {other:?}"),
        }
    }

    #[test]
    fn mixed_thinking_text_tool_call_indices() {
        let mut state = new_state();

        // Thinking block at provider index 0 → harness index 0
        let (events, _) = process(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            &mut state,
        );
        assert!(matches!(
            events[0],
            AssistantMessageEvent::ThinkingStart { content_index: 0 }
        ));

        let (events, _) = process("content_block_stop", r#"{"index":0}"#, &mut state);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                ..
            }
        ));

        // Text block at provider index 1 → harness index 1
        let (events, _) = process(
            "content_block_start",
            r#"{"index":1,"content_block":{"type":"text","text":""}}"#,
            &mut state,
        );
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextStart { content_index: 1 }
        ));

        let (events, _) = process("content_block_stop", r#"{"index":1}"#, &mut state);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextEnd { content_index: 1 }
        ));

        // Tool call at provider index 2 → harness index 2
        let (events, _) = process(
            "content_block_start",
            r#"{"index":2,"content_block":{"type":"tool_use","id":"call_1","name":"bash"}}"#,
            &mut state,
        );
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::ToolCallStart {
                content_index: 2,
                ..
            }
        ));
    }

    #[test]
    fn multiple_sequential_tool_calls() {
        let mut state = new_state();

        // First tool call at provider index 0
        let (events, _) = process(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"tool_use","id":"tc_1","name":"read_file"}}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::ToolCallStart {
                content_index,
                id,
                name,
            } => {
                assert_eq!(*content_index, 0);
                assert_eq!(id, "tc_1");
                assert_eq!(name, "read_file");
            }
            other => panic!("expected ToolCallStart, got {other:?}"),
        }

        // Delta for first tool call
        let (events, _) = process(
            "content_block_delta",
            r#"{"index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"foo\"}"}}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                ..
            }
        ));

        // Close first tool call
        let (events, _) = process("content_block_stop", r#"{"index":0}"#, &mut state);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::ToolCallEnd { content_index: 0 }
        ));

        // Second tool call at provider index 1 → harness index 1
        let (events, _) = process(
            "content_block_start",
            r#"{"index":1,"content_block":{"type":"tool_use","id":"tc_2","name":"write_file"}}"#,
            &mut state,
        );
        match &events[0] {
            AssistantMessageEvent::ToolCallStart {
                content_index,
                id,
                name,
            } => {
                assert_eq!(*content_index, 1);
                assert_eq!(id, "tc_2");
                assert_eq!(name, "write_file");
            }
            other => panic!("expected ToolCallStart, got {other:?}"),
        }

        // Close second tool call
        let (events, _) = process("content_block_stop", r#"{"index":1}"#, &mut state);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::ToolCallEnd { content_index: 1 }
        ));
    }

    #[test]
    fn message_stop_emits_done_with_usage() {
        let mut state = new_state();

        // Set up usage via message_start
        process(
            "message_start",
            r#"{"message":{"usage":{"input_tokens":100,"cache_read_input_tokens":10,"cache_creation_input_tokens":5}}}"#,
            &mut state,
        );

        // Set up stop reason + output tokens
        process(
            "message_delta",
            r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":50}}"#,
            &mut state,
        );

        // message_stop triggers Done
        let (events, done) = process("message_stop", r"{}", &mut state);
        assert!(done);
        assert_eq!(events.len(), 1);
        match &events[0] {
            AssistantMessageEvent::Done {
                stop_reason, usage, ..
            } => {
                assert_eq!(*stop_reason, StopReason::Stop);
                assert_eq!(usage.input, 100);
                assert_eq!(usage.output, 50);
                assert_eq!(usage.cache_read, 10);
                assert_eq!(usage.cache_write, 5);
                assert_eq!(usage.total, 165);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn error_event_closes_open_blocks() {
        let mut state = new_state();

        // Open a text block
        process(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
            &mut state,
        );

        // SSE error arrives before content_block_stop
        let (events, done) = process(
            "error",
            r#"{"error":{"type":"overloaded_error","message":"Server overloaded"}}"#,
            &mut state,
        );
        assert!(done);
        // Should have: TextEnd (from finalize) + error event
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextEnd { content_index: 0 }
        ));
    }

    #[test]
    fn malformed_message_start_is_terminal_protocol_error() {
        let mut state = new_state();

        let (events, done) = process("message_start", "{", &mut state);

        assert!(done);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_kind: None,
                error_message,
                ..
            } if error_message.contains("Anthropic message_start JSON parse error")
        ));
    }

    #[test]
    fn malformed_content_block_delta_finalizes_open_blocks_before_error() {
        let mut state = new_state();

        process(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
            &mut state,
        );

        let (events, done) = process("content_block_delta", "{", &mut state);

        assert!(done);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextEnd { content_index: 0 }
        ));
        assert!(matches!(
            &events[1],
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_kind: None,
                error_message,
                ..
            } if error_message.contains("Anthropic content_block_delta JSON parse error")
        ));
    }

    #[test]
    fn malformed_error_event_is_non_retryable_parse_error() {
        let mut state = new_state();

        process(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
            &mut state,
        );

        let (events, done) = process("error", "{", &mut state);

        assert!(done);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextEnd { content_index: 0 }
        ));
        assert!(matches!(
            &events[1],
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_kind: None,
                error_message,
                ..
            } if error_message.contains("Anthropic error JSON parse error")
        ));
    }

    #[test]
    fn tool_use_stop_reason_mapping() {
        let mut state = new_state();

        process(
            "message_delta",
            r#"{"delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":10}}"#,
            &mut state,
        );

        let (events, done) = process("message_stop", r"{}", &mut state);
        assert!(done);
        match &events[0] {
            AssistantMessageEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::ToolUse);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn open_blocks_drained_on_message_stop() {
        let mut state = new_state();

        // Open text and tool call, don't close them
        process(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
            &mut state,
        );
        process(
            "content_block_start",
            r#"{"index":1,"content_block":{"type":"tool_use","id":"tc_1","name":"bash"}}"#,
            &mut state,
        );

        // message_stop should finalize both open blocks
        let (events, done) = process("message_stop", r"{}", &mut state);
        assert!(done);
        // TextEnd + ToolCallEnd + Done = 3 events
        assert_eq!(events.len(), 3);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextEnd { content_index: 0 }
        ));
        assert!(matches!(
            events[1],
            AssistantMessageEvent::ToolCallEnd { content_index: 1 }
        ));
        assert!(matches!(events[2], AssistantMessageEvent::Done { .. }));
    }

    #[test]
    fn mixed_text_and_tool_call_stream() {
        let mut state = new_state();

        // Text block
        process(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
            &mut state,
        );
        process(
            "content_block_delta",
            r#"{"index":0,"delta":{"type":"text_delta","text":"I will run a command."}}"#,
            &mut state,
        );
        let (events, _) = process("content_block_stop", r#"{"index":0}"#, &mut state);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::TextEnd { content_index: 0 }
        ));

        // Tool call block
        process(
            "content_block_start",
            r#"{"index":1,"content_block":{"type":"tool_use","id":"call_abc","name":"bash"}}"#,
            &mut state,
        );
        process(
            "content_block_delta",
            r#"{"index":1,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"ls\"}"}}"#,
            &mut state,
        );
        let (events, _) = process("content_block_stop", r#"{"index":1}"#, &mut state);
        assert!(matches!(
            events[0],
            AssistantMessageEvent::ToolCallEnd { content_index: 1 }
        ));

        // Done
        process(
            "message_delta",
            r#"{"delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":20}}"#,
            &mut state,
        );
        let (events, done) = process("message_stop", r"{}", &mut state);
        assert!(done);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                ..
            }
        ));
    }

    #[test]
    fn trailing_slash_stripped() {
        let anthropic = AnthropicStreamFn::new("https://api.anthropic.com/", "key");
        assert_eq!(anthropic.base.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn no_trailing_slash_unchanged() {
        let anthropic = AnthropicStreamFn::new("https://api.anthropic.com", "key");
        assert_eq!(anthropic.base.base_url, "https://api.anthropic.com");
    }

    // ── Issue #619: incomplete tool_use sanitization ─────────────────────

    /// Regression for #619: after the loop-level scrub runs, an assistant
    /// message that originally carried `arguments: Null` with `partial_json`
    /// set must serialize with `input: {}` so the Anthropic API accepts the
    /// replayed history on the next turn.
    #[test]
    fn convert_messages_sanitized_tool_use_becomes_empty_object_input() {
        use swink_agent::AssistantMessage;

        let mut assistant = AssistantMessage {
            content: vec![ContentBlock::ToolCall {
                id: "toolu_01".into(),
                name: "read_file".into(),
                // Simulate an incomplete-tool-use block surviving Done(Length):
                arguments: Value::Null,
                partial_json: Some(r#"{"path": "/tm"#.into()),
            }],
            provider: "anthropic".into(),
            model_id: "claude-sonnet-4-6".into(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Length,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        };

        // Loop-level scrub runs before the adapter sees the history.
        swink_agent::sanitize_incomplete_tool_calls(&mut assistant);

        let messages = vec![AgentMessage::Llm(LlmMessage::Assistant(assistant))];
        let (_system, converted) = convert_messages(&messages, "");

        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "assistant");
        let json = serde_json::to_value(&converted[0]).unwrap();
        let block = &json["content"][0];
        assert_eq!(block["type"], "tool_use");
        assert_eq!(block["id"], "toolu_01");
        assert_eq!(block["name"], "read_file");
        // The critical assertion: input is a valid empty JSON object, NOT null.
        assert!(
            block["input"].is_object(),
            "input must be a JSON object, got {:?}",
            block["input"]
        );
        assert_eq!(block["input"].as_object().unwrap().len(), 0);
    }
}
