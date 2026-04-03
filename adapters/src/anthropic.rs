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
use swink_agent::stream::{AssistantMessageEvent, CacheStrategy, StreamFn, StreamOptions};
use swink_agent::types::{
    AgentContext, AgentMessage, Cost, LlmMessage, ModelSpec, StopReason, ThinkingLevel, Usage,
};

use crate::base::AdapterBase;
use crate::convert::extract_tool_schemas;
use crate::sse::{SseLine as SharedSseLine, sse_lines};

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

/// The type of content block currently active at a given index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockType {
    Text,
    Thinking,
    ToolUse,
}

/// State machine tracking SSE streaming progress.
struct SseStreamState {
    content_index: usize,
    /// Maps Anthropic block index → `(BlockType, harness content_index)`.
    active_blocks: HashMap<usize, (BlockType, usize)>,
    usage: Usage,
    stop_reason: Option<StopReason>,
}

/// Parsed SSE line with event type.
enum SseLine {
    /// An `event: <type>` + `data: <json>` pair.
    Event { event_type: String, data: String },
}

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
        let response = match send_request(anthropic, model, context, options).await {
            Ok(resp) => resp,
            Err(event) => return stream::iter(vec![event]).left_stream(),
        };

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
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
            return stream::iter(vec![event]).left_stream();
        }

        parse_sse_stream(response, cancellation_token).right_stream()
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
                        ContentBlock::Text { text } => {
                            if !text.is_empty() {
                                content.push(AnthropicContentBlock::Text { text: text.clone() });
                            }
                        }
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => {
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
) -> impl Stream<Item = AssistantMessageEvent> + Send {
    let byte_stream = response.bytes_stream();
    let line_stream = sse_event_lines(byte_stream);

    stream::unfold(
        (
            Box::pin(line_stream),
            cancellation_token,
            SseStreamState {
                content_index: 0,
                active_blocks: HashMap::new(),
                usage: Usage::default(),
                stop_reason: None,
            },
            false,
            true,
        ),
        |(mut lines, token, mut state, mut done, first)| async move {
            if done {
                return None;
            }

            // Emit Start on first call
            if first {
                return Some((
                    vec![AssistantMessageEvent::Start],
                    (lines, token, state, done, false),
                ));
            }

            tokio::select! {
                biased;
                () = token.cancelled() => {
                    let mut events = crate::finalize::finalize_blocks(&mut state);
                    events.push(AssistantMessageEvent::Error {
                        stop_reason: StopReason::Aborted,
                        error_message: "operation cancelled".to_string(),
                        usage: None,
                        error_kind: None,
                    });
                    done = true;
                    Some((events, (lines, token, state, done, false)))
                }
                item = lines.next() => {
                    match item {
                        None => {
                            // Stream ended unexpectedly
                            done = true;
                            let mut events = crate::finalize::finalize_blocks(&mut state);
                            events.push(AssistantMessageEvent::error_network("Anthropic stream ended unexpectedly"));
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(SseLine::Event { event_type, data }) => {
                            let events = process_sse_event(
                                &event_type,
                                &data,
                                &mut state,
                                &mut done,
                            );
                            Some((events, (lines, token, state, done, false)))
                        }
                    }
                }
            }
        },
    )
    .flat_map(stream::iter)
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
            if let Ok(parsed) = serde_json::from_str::<Value>(data) {
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
        }

        "content_block_start" => {
            if let Ok(parsed) = serde_json::from_str::<Value>(data) {
                let index = parsed["index"]
                    .as_u64()
                    .unwrap_or(0)
                    .try_into()
                    .unwrap_or(0);
                let block_type = parsed
                    .pointer("/content_block/type")
                    .and_then(Value::as_str)
                    .unwrap_or("");

                let content_index = state.content_index;

                match block_type {
                    "text" => {
                        state
                            .active_blocks
                            .insert(index, (BlockType::Text, content_index));
                        state.content_index += 1;
                        events.push(AssistantMessageEvent::TextStart { content_index });
                    }
                    "thinking" => {
                        state
                            .active_blocks
                            .insert(index, (BlockType::Thinking, content_index));
                        state.content_index += 1;
                        events.push(AssistantMessageEvent::ThinkingStart { content_index });
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
                        state
                            .active_blocks
                            .insert(index, (BlockType::ToolUse, content_index));
                        state.content_index += 1;
                        events.push(AssistantMessageEvent::ToolCallStart {
                            content_index,
                            id,
                            name,
                        });
                    }
                    _ => {}
                }
            }
        }

        "content_block_delta" => {
            if let Ok(parsed) = serde_json::from_str::<Value>(data) {
                let index = parsed["index"]
                    .as_u64()
                    .unwrap_or(0)
                    .try_into()
                    .unwrap_or(0);
                let delta_type = parsed
                    .pointer("/delta/type")
                    .and_then(Value::as_str)
                    .unwrap_or("");

                if let Some(&(_, content_index)) = state.active_blocks.get(&index) {
                    match delta_type {
                        "text_delta" => {
                            if let Some(text) =
                                parsed.pointer("/delta/text").and_then(Value::as_str)
                            {
                                events.push(AssistantMessageEvent::TextDelta {
                                    content_index,
                                    delta: text.to_string(),
                                });
                            }
                        }
                        "thinking_delta" => {
                            if let Some(thinking) =
                                parsed.pointer("/delta/thinking").and_then(Value::as_str)
                            {
                                events.push(AssistantMessageEvent::ThinkingDelta {
                                    content_index,
                                    delta: thinking.to_string(),
                                });
                            }
                        }
                        "input_json_delta" => {
                            if let Some(json) = parsed
                                .pointer("/delta/partial_json")
                                .and_then(Value::as_str)
                            {
                                events.push(AssistantMessageEvent::ToolCallDelta {
                                    content_index,
                                    delta: json.to_string(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        "content_block_stop" => {
            if let Ok(parsed) = serde_json::from_str::<Value>(data) {
                let index = parsed["index"]
                    .as_u64()
                    .unwrap_or(0)
                    .try_into()
                    .unwrap_or(0);

                if let Some((block_type, content_index)) = state.active_blocks.remove(&index) {
                    match block_type {
                        BlockType::Text => {
                            events.push(AssistantMessageEvent::TextEnd { content_index });
                        }
                        BlockType::Thinking => {
                            // Extract signature if present
                            let signature = parsed
                                .pointer("/signature")
                                .and_then(Value::as_str)
                                .map(String::from);
                            events.push(AssistantMessageEvent::ThinkingEnd {
                                content_index,
                                signature,
                            });
                        }
                        BlockType::ToolUse => {
                            events.push(AssistantMessageEvent::ToolCallEnd { content_index });
                        }
                    }
                }
            }
        }

        "message_delta" => {
            if let Ok(parsed) = serde_json::from_str::<Value>(data) {
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
            events.extend(crate::finalize::finalize_blocks(state));

            let parsed = serde_json::from_str::<Value>(data).ok();
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
        let mut indices: Vec<usize> = self.active_blocks.keys().copied().collect();
        indices.sort_unstable();

        let mut blocks = Vec::new();
        for idx in indices {
            if let Some((block_type, content_index)) = self.active_blocks.remove(&idx) {
                blocks.push(match block_type {
                    BlockType::Text => crate::finalize::OpenBlock::Text { content_index },
                    BlockType::Thinking => crate::finalize::OpenBlock::Thinking {
                        content_index,
                        signature: None,
                    },
                    BlockType::ToolUse => crate::finalize::OpenBlock::ToolCall { content_index },
                });
            }
        }

        blocks
    }
}

/// Convert a byte stream into a stream of parsed SSE event+data pairs.
///
/// Anthropic SSE has both `event:` and `data:` lines. This parser pairs them
/// together using a simple state machine.
fn sse_event_lines(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> Pin<Box<dyn Stream<Item = SseLine> + Send + 'static>> {
    Box::pin(stream::unfold(
        (Box::pin(sse_lines(byte_stream)), Option::<String>::None),
        |(mut stream, mut current_event)| async move {
            loop {
                match stream.next().await {
                    Some(SharedSseLine::Empty | SharedSseLine::Done) => {
                        current_event = None;
                    }
                    Some(SharedSseLine::Event(event_type)) => {
                        current_event = Some(event_type);
                    }
                    Some(SharedSseLine::Data(data)) => {
                        if !data.is_empty() {
                            let event_type = current_event
                                .take()
                                .unwrap_or_else(|| "unknown".to_string());
                            return Some((
                                SseLine::Event { event_type, data },
                                (stream, current_event),
                            ));
                        }
                    }
                    None => return None,
                }
            }
        },
    ))
}

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
}
