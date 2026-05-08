//! Shared OpenAI-compatible request/response types.
//!
//! Azure, Mistral, xAI, and plain `OpenAI` all use structurally identical
//! message, tool, and streaming chunk types. This module defines them once
//! so every adapter can reuse them without copy-paste.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::error;

use swink_agent::AgentTool;
use swink_agent::ContentBlock;
use swink_agent::{
    AssistantMessage as HarnessAssistantMessage, AssistantMessageEvent, Cost, StopReason,
    ToolResultMessage, Usage, UserMessage,
};

use crate::convert::{MessageConverter, extract_tool_schemas};
use crate::sse::{SseAction, SseLine, sse_data_lines_with_callback};

// ─── Request types ──────────────────────────────────────────────────────────

/// Message in `OpenAI`'s chat completions format.
#[derive(Debug, Serialize)]
pub struct OaiMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OaiToolCallRequest>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Tool call in the request (assistant message replay).
#[derive(Debug, Serialize)]
pub struct OaiToolCallRequest {
    pub id: String,
    pub r#type: String,
    pub function: OaiFunctionCallRequest,
}

/// Function call details in a request tool call.
#[derive(Debug, Serialize)]
pub struct OaiFunctionCallRequest {
    pub name: String,
    pub arguments: String,
}

/// Tool definition in `OpenAI`'s format.
#[derive(Debug, Serialize)]
pub struct OaiTool {
    pub r#type: String,
    pub function: OaiToolDef,
}

/// Tool function definition.
#[derive(Debug, Serialize)]
pub struct OaiToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Stream options for the request.
#[derive(Debug, Serialize)]
pub struct OaiStreamOptions {
    pub include_usage: bool,
}

/// Full request body for OpenAI-compatible `/v1/chat/completions`.
#[derive(Debug, Serialize)]
pub struct OaiChatRequest {
    pub model: String,
    pub messages: Vec<OaiMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<OaiStreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OaiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
}

// ─── Response / streaming types ─────────────────────────────────────────────

/// A single SSE chunk from an OpenAI-compatible streaming response.
#[derive(Deserialize)]
pub struct OaiChunk {
    #[serde(default)]
    pub choices: Vec<OaiChoice>,
    #[serde(default)]
    pub usage: Option<OaiUsage>,
}

/// A choice in a streaming chunk.
#[derive(Deserialize)]
pub struct OaiChoice {
    #[serde(default)]
    pub delta: OaiDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub content_filter_results: Option<Value>,
}

/// The delta portion of a streaming choice.
#[derive(Default, Deserialize)]
pub struct OaiDelta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<OaiToolCallDelta>>,
    /// Reasoning/thinking content emitted by vLLM and other OpenAI-compatible
    /// servers when serving thinking-capable models.
    #[serde(default)]
    pub reasoning_content: Option<String>,
}

/// A tool call delta in a streaming response.
#[derive(Deserialize)]
pub struct OaiToolCallDelta {
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<OaiFunctionDelta>,
}

/// Function delta in a tool call.
#[derive(Deserialize)]
pub struct OaiFunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

/// Usage information in the response.
#[derive(Deserialize)]
pub struct OaiUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl OaiUsage {
    fn to_usage(&self) -> Usage {
        let mut extra = HashMap::new();
        for (key, value) in &self.extra {
            collect_numeric_usage_fields(key.clone(), value, &mut extra);
        }

        Usage {
            input: self.prompt_tokens,
            output: self.completion_tokens,
            cache_read: 0,
            cache_write: 0,
            total: self
                .total_tokens
                .unwrap_or(self.prompt_tokens + self.completion_tokens),
            extra,
        }
    }
}

fn collect_numeric_usage_fields(key: String, value: &Value, extra: &mut HashMap<String, u64>) {
    match value {
        Value::Number(number) => {
            if let Some(value) = number.as_u64() {
                extra.insert(key, value);
            }
        }
        Value::Object(fields) => {
            for (child_key, child_value) in fields {
                collect_numeric_usage_fields(format!("{key}.{child_key}"), child_value, extra);
            }
        }
        _ => {}
    }
}

// ─── Tool call state tracking ───────────────────────────────────────────────

/// Tracks OAI-specific per-tool-call streaming state.
///
/// The `content_index` is the harness-side index allocated by
/// [`BlockAccumulator`] when the tool call was first opened.  `arguments`
/// accumulates the partial JSON across deltas.
pub struct OaiToolCallEntry {
    pub id: String,
    pub name: Option<String>,
    pub arguments: String,
    pub content_index: Option<usize>,
}

// ─── MessageConverter impl ──────────────────────────────────────────────────

/// Marker type for OpenAI-compatible message conversion.
///
/// Reused by any adapter whose wire format matches the `OpenAI` chat completions
/// message schema (`OpenAI`, Azure, Mistral, xAI, etc.).
pub struct OaiConverter;

impl MessageConverter for OaiConverter {
    type Message = OaiMessage;

    fn system_message(system_prompt: &str) -> Option<OaiMessage> {
        Some(OaiMessage {
            role: "system".to_string(),
            content: Some(system_prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
        })
    }

    fn user_message(user: &UserMessage) -> OaiMessage {
        let content = ContentBlock::extract_text(&user.content);
        OaiMessage {
            role: "user".to_string(),
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn assistant_message(assistant: &HarnessAssistantMessage) -> OaiMessage {
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        for block in &assistant.content {
            match block {
                ContentBlock::Text { text } => {
                    content.push_str(text);
                }
                ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    // Issue #619: loop-level scrub coerces incomplete tool-use blocks
                    // into object-typed arguments before reaching here. Debug builds
                    // assert the invariant to catch regressions. OpenAI-compat is the
                    // most dangerous case: `Value::Null.to_string()` is the literal
                    // string "null", which the provider accepts structurally but then
                    // rejects when parsing arguments.
                    debug_assert!(
                        arguments.is_object(),
                        "openai-compat adapter: function.arguments must stringify from a JSON object (got {arguments:?}); loop-level sanitize_incomplete_tool_calls should have coerced this before dispatch"
                    );
                    tool_calls.push(OaiToolCallRequest {
                        id: id.clone(),
                        r#type: "function".to_string(),
                        function: OaiFunctionCallRequest {
                            name: name.clone(),
                            arguments: arguments.to_string(),
                        },
                    });
                }
                _ => {}
            }
        }
        OaiMessage {
            role: "assistant".to_string(),
            content: if content.is_empty() {
                None
            } else {
                Some(content)
            },
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            tool_call_id: None,
        }
    }

    fn tool_result_message(result: &ToolResultMessage) -> OaiMessage {
        let content = ContentBlock::extract_text(&result.content);
        OaiMessage {
            role: "tool".to_string(),
            content: Some(content),
            tool_calls: None,
            tool_call_id: Some(result.tool_call_id.clone()),
        }
    }
}

// ─── Shared helpers ─────────────────────────────────────────────────────────

/// Build the `tools` vec and `tool_choice` from the agent context's tool list.
pub fn build_oai_tools(tools: &[Arc<dyn AgentTool>]) -> (Vec<OaiTool>, Option<String>) {
    let oai_tools: Vec<OaiTool> = extract_tool_schemas(tools)
        .into_iter()
        .map(|s| OaiTool {
            r#type: "function".to_string(),
            function: OaiToolDef {
                name: s.name,
                description: s.description,
                parameters: s.parameters,
            },
        })
        .collect();
    let tool_choice = if oai_tools.is_empty() {
        None
    } else {
        Some("auto".to_string())
    };
    (oai_tools, tool_choice)
}

// ─── Shared OAI-compatible SSE stream parsing ──────────────────────────────

/// State machine tracking SSE streaming progress for OAI-compatible
/// adapters (`OpenAI`, Azure, Mistral, xAI, etc.).
///
/// Text and tool-call block lifecycle (index allocation, open/close tracking,
/// and stream-end draining) is delegated to [`BlockAccumulator`].  The
/// `tool_calls` map is keyed by the **provider-side chunk index** (0-based
/// sequential index within the OAI streaming response) and holds only the
/// accumulated arguments alongside the harness content index that
/// [`BlockAccumulator`] assigned when the tool call was first seen.
#[derive(Default)]
pub struct OaiSseStreamState {
    pub blocks: crate::block_accumulator::BlockAccumulator,
    /// Provider-index → (arguments, harness `content_index`).
    pub tool_calls: HashMap<usize, OaiToolCallEntry>,
    pub usage: Option<Usage>,
    /// Saved stop reason from `finish_reason`; emitted with `Done` on `[DONE]`.
    pub stop_reason: Option<StopReason>,
    /// Terminal provider error captured from a finish reason that should not
    /// be downgraded into a normal `Done` event.
    pub terminal_error: Option<AssistantMessageEvent>,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct OaiParserOptions {
    pub(crate) detect_content_filter_results: bool,
}

impl crate::finalize::StreamFinalize for OaiSseStreamState {
    fn drain_open_blocks(&mut self) -> Vec<crate::finalize::OpenBlock> {
        // Tool-call entries in the HashMap that were opened in `blocks` will be
        // drained by the accumulator; we only need to remove our own bookkeeping.
        self.tool_calls.clear();
        crate::finalize::StreamFinalize::drain_open_blocks(&mut self.blocks)
    }
}

impl OaiSseStreamState {
    fn emit_terminal_error(&mut self, event: AssistantMessageEvent) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        let _ = flush_pending_oai_tool_calls(self, &mut events, "OpenAI-compatible");
        events.extend(crate::finalize::finalize_blocks(self));
        events.push(event);
        events
    }

    fn emit_done_from_done_sentinel(&mut self) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        if let Some(error) = flush_pending_oai_tool_calls(self, &mut events, "OpenAI-compatible") {
            events.extend(crate::finalize::finalize_blocks(self));
            events.push(error);
            return events;
        }
        events.extend(crate::finalize::finalize_blocks(self));
        if let Some(error) = self.terminal_error.take() {
            events.push(error);
        } else {
            let stop_reason = self.stop_reason.take().unwrap_or(StopReason::Stop);
            let usage = self.usage.take().unwrap_or_default();
            events.push(AssistantMessageEvent::Done {
                stop_reason,
                usage,
                cost: Cost::default(),
            });
        }
        events
    }
}

/// Process a single deserialized `OaiChunk`, updating state and emitting events.
///
/// This is the shared chunk-processing logic used by both `OpenAI` and Azure
/// adapters. The `provider` label is used for fallback tool-call IDs.
#[allow(dead_code)]
pub fn process_oai_chunk(
    chunk: &OaiChunk,
    state: &mut OaiSseStreamState,
    events: &mut Vec<AssistantMessageEvent>,
    provider: &str,
) {
    process_oai_chunk_with_options(chunk, state, events, provider, OaiParserOptions::default());
}

fn process_oai_chunk_with_options(
    chunk: &OaiChunk,
    state: &mut OaiSseStreamState,
    events: &mut Vec<AssistantMessageEvent>,
    provider: &str,
    options: OaiParserOptions,
) {
    if let Some(u) = &chunk.usage {
        state.usage = Some(u.to_usage());
    }

    for choice in &chunk.choices {
        // ── Reasoning / thinking content (vLLM, etc.) ──────────────────
        if let Some(reasoning) = &choice.delta.reasoning_content
            && !reasoning.is_empty()
        {
            if let Some(ev) = state.blocks.ensure_thinking_open() {
                events.push(ev);
            }
            if let Some(ev) = state.blocks.thinking_delta(reasoning.clone()) {
                events.push(ev);
            }
        }

        // ── Regular text content ───────────────────────────────────────
        if let Some(content) = &choice.delta.content
            && !content.is_empty()
        {
            // Transition from thinking → text: close the thinking block.
            if let Some(ev) = state.blocks.close_thinking(None) {
                events.push(ev);
            }
            if let Some(ev) = state.blocks.ensure_text_open() {
                events.push(ev);
            }
            if let Some(ev) = state.blocks.text_delta(content.clone()) {
                events.push(ev);
            }
        }

        // ── Tool calls ────────────────────────────────────────────────
        if let Some(tool_calls) = &choice.delta.tool_calls {
            // Close thinking if still open when tool calls arrive.
            if let Some(ev) = state.blocks.close_thinking(None) {
                events.push(ev);
            }
            if let Some(ev) = state.blocks.close_text() {
                events.push(ev);
            }

            for tc_delta in tool_calls {
                process_oai_tool_call_delta(tc_delta, state, events, provider);
            }
        }

        if options.detect_content_filter_results
            && choice
                .content_filter_results
                .as_ref()
                .is_some_and(content_filter_results_filtered)
        {
            let _ = flush_pending_oai_tool_calls(state, events, provider);
            events.extend(crate::finalize::finalize_blocks(state));
            state.terminal_error = Some(AssistantMessageEvent::error_content_filtered(format!(
                "{provider} response stopped by content filter"
            )));
            return;
        }

        if let Some(reason) = &choice.finish_reason {
            if reason == "content_filter" {
                let _ = flush_pending_oai_tool_calls(state, events, provider);
                events.extend(crate::finalize::finalize_blocks(state));
                state.terminal_error = Some(AssistantMessageEvent::error_content_filtered(
                    format!("{provider} response stopped by content filter"),
                ));
                return;
            }

            if provider == "Mistral" && reason == "error" {
                let _ = flush_pending_oai_tool_calls(state, events, provider);
                events.extend(crate::finalize::finalize_blocks(state));
                state.terminal_error = Some(AssistantMessageEvent::Error {
                    stop_reason: StopReason::Error,
                    error_message: "Mistral reported finish_reason=error".to_string(),
                    usage: state.usage.clone(),
                    error_kind: None,
                });
                return;
            }

            let stop_reason = match reason.as_str() {
                "tool_calls" => StopReason::ToolUse,
                "length" | "model_length" => StopReason::Length,
                _ => StopReason::Stop,
            };

            if let Some(error) = flush_pending_oai_tool_calls(state, events, provider) {
                events.extend(crate::finalize::finalize_blocks(state));
                state.terminal_error = Some(error);
                return;
            }
            events.extend(crate::finalize::finalize_blocks(state));
            state.stop_reason = Some(stop_reason);
        }
    }
}

fn content_filter_results_filtered(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            matches!(map.get("filtered"), Some(Value::Bool(true)))
                || map.values().any(content_filter_results_filtered)
        }
        Value::Array(values) => values.iter().any(content_filter_results_filtered),
        _ => false,
    }
}

/// Process a single tool call delta, updating state and emitting events.
fn process_oai_tool_call_delta(
    tc_delta: &OaiToolCallDelta,
    state: &mut OaiSseStreamState,
    events: &mut Vec<AssistantMessageEvent>,
    provider: &str,
) {
    let tc_index = tc_delta.index;
    let mut emit_delta = None;
    let mut open_tool_call = None;

    {
        let tc_entry = state
            .tool_calls
            .entry(tc_index)
            .or_insert_with(|| OaiToolCallEntry {
                id: tc_delta
                    .id
                    .clone()
                    .unwrap_or_else(|| format!("{provider}-tool-{tc_index}")),
                name: None,
                arguments: String::new(),
                content_index: None,
            });

        if tc_entry.content_index.is_none()
            && let Some(id) = &tc_delta.id
        {
            tc_entry.id.clone_from(id);
        }

        if let Some(name) = tc_delta
            .function
            .as_ref()
            .and_then(|f| f.name.as_ref())
            .filter(|name| !name.is_empty())
        {
            tc_entry.name = Some(name.clone());
        }

        if let Some(args) = tc_delta
            .function
            .as_ref()
            .and_then(|f| f.arguments.as_ref())
            && !args.is_empty()
        {
            tc_entry.arguments.push_str(args);
            if let Some(content_index) = tc_entry.content_index {
                emit_delta = Some((content_index, args.clone()));
            }
        }

        if tc_entry.content_index.is_none()
            && let Some(name) = tc_entry.name.clone()
        {
            open_tool_call = Some((tc_entry.id.clone(), name, tc_entry.arguments.clone()));
        }
    }

    if let Some((id, name, buffered_arguments)) = open_tool_call {
        let (content_index, start_ev) = state.blocks.open_tool_call(id, name);
        events.push(start_ev);

        if !buffered_arguments.is_empty() {
            events.push(crate::block_accumulator::BlockAccumulator::tool_call_delta(
                content_index,
                buffered_arguments,
            ));
        }

        let tc_entry = state
            .tool_calls
            .get_mut(&tc_index)
            .expect("entry exists after opening");
        tc_entry.content_index = Some(content_index);
        return;
    }

    if let Some((content_index, args)) = emit_delta {
        events.push(crate::block_accumulator::BlockAccumulator::tool_call_delta(
            content_index,
            args,
        ));
    }
}

fn flush_pending_oai_tool_calls(
    state: &mut OaiSseStreamState,
    events: &mut Vec<AssistantMessageEvent>,
    provider: &str,
) -> Option<AssistantMessageEvent> {
    let mut pending_indices: Vec<_> = state
        .tool_calls
        .iter()
        .filter_map(|(tc_index, entry)| entry.content_index.is_none().then_some(*tc_index))
        .collect();
    pending_indices.sort_unstable();

    for tc_index in pending_indices {
        let pending_entry = {
            let entry = state
                .tool_calls
                .get(&tc_index)
                .expect("pending entry should exist");
            (
                entry.id.clone(),
                entry.name.clone().filter(|name| !name.is_empty()),
                entry.arguments.clone(),
            )
        };
        let (id, name, arguments) = match pending_entry {
            (id, Some(name), arguments) => (id, name, arguments),
            (id, None, _) => {
                state.tool_calls.clear();
                return Some(AssistantMessageEvent::error(format!(
                    "{provider} stream ended with incomplete tool call {id}: missing function name",
                )));
            }
        };

        let (content_index, start_ev) = state.blocks.open_tool_call(id, name);
        events.push(start_ev);

        if !arguments.is_empty() {
            events.push(crate::block_accumulator::BlockAccumulator::tool_call_delta(
                content_index,
                arguments,
            ));
        }

        let entry = state
            .tool_calls
            .get_mut(&tc_index)
            .expect("pending entry should still exist");
        entry.content_index = Some(content_index);
    }

    None
}

/// Parse an OpenAI-compatible SSE streaming response into `AssistantMessageEvent`
/// values.
///
/// This is the shared SSE state machine used by `OpenAI`, Azure, and other
/// OAI-compatible adapters. The `provider` label is used in error messages
/// and fallback tool-call IDs.
#[allow(clippy::too_many_lines)]
#[allow(dead_code)]
pub fn parse_oai_sse_stream(
    response: reqwest::Response,
    cancellation_token: CancellationToken,
    provider: &'static str,
    on_raw_payload: Option<swink_agent::OnRawPayload>,
) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>> {
    parse_oai_sse_stream_with_options(
        response,
        cancellation_token,
        provider,
        on_raw_payload,
        OaiParserOptions::default(),
    )
}

pub(crate) fn parse_oai_sse_stream_with_options(
    response: reqwest::Response,
    cancellation_token: CancellationToken,
    provider: &'static str,
    on_raw_payload: Option<swink_agent::OnRawPayload>,
    options: OaiParserOptions,
) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>> {
    let line_stream = sse_data_lines_with_callback(response.bytes_stream(), on_raw_payload);

    crate::sse::sse_adapter_stream(
        line_stream,
        cancellation_token,
        OaiSseStreamState::default(),
        "operation cancelled",
        move |item, state| match item {
            None => {
                if let Some(error) = state.terminal_error.take() {
                    return SseAction::Done(state.emit_terminal_error(error));
                }
                SseAction::Done(
                    state.emit_terminal_error(AssistantMessageEvent::error_network(format!(
                        "{provider} stream ended unexpectedly"
                    ))),
                )
            }
            Some(SseLine::Done) => SseAction::Done(state.emit_done_from_done_sentinel()),
            Some(SseLine::Data(data)) => {
                let chunk: OaiChunk = match serde_json::from_str(&data) {
                    Ok(c) => c,
                    Err(e) => {
                        error!(error = %e, "{provider} JSON parse error");
                        return SseAction::Done(state.emit_terminal_error(
                            AssistantMessageEvent::error(format!(
                                "{provider} JSON parse error: {e}",
                            )),
                        ));
                    }
                };

                let mut events = Vec::new();
                process_oai_chunk_with_options(&chunk, state, &mut events, provider, options);
                if let Some(error) = state.terminal_error.take() {
                    events.push(error);
                    SseAction::Done(events)
                } else {
                    SseAction::Continue(events)
                }
            }
            Some(SseLine::TransportError(message)) => SseAction::Done(state.emit_terminal_error(
                AssistantMessageEvent::error_network(format!("{provider} {message}")),
            )),
            Some(SseLine::ProtocolError(message)) => SseAction::Done(state.emit_terminal_error(
                AssistantMessageEvent::error(format!("{provider} {message}")),
            )),
            Some(_) => SseAction::Skip,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create an `OaiChunk` with one choice containing the given delta.
    fn chunk_with_delta(delta: OaiDelta, finish_reason: Option<&str>) -> OaiChunk {
        OaiChunk {
            choices: vec![OaiChoice {
                delta,
                finish_reason: finish_reason.map(String::from),
                content_filter_results: None,
            }],
            usage: None,
        }
    }

    #[test]
    fn reasoning_content_emits_thinking_events() {
        let mut state = OaiSseStreamState::default();
        let mut events = Vec::new();

        // First reasoning chunk → ThinkingStart + ThinkingDelta
        let chunk = chunk_with_delta(
            OaiDelta {
                reasoning_content: Some("Let me think".to_string()),
                ..Default::default()
            },
            None,
        );
        process_oai_chunk(&chunk, &mut state, &mut events, "test");

        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::ThinkingStart { content_index: 0 }
        ));
        assert!(
            matches!(&events[1], AssistantMessageEvent::ThinkingDelta { content_index: 0, delta } if delta == "Let me think")
        );

        // Second reasoning chunk → only ThinkingDelta (no new Start)
        events.clear();
        let chunk = chunk_with_delta(
            OaiDelta {
                reasoning_content: Some(" more".to_string()),
                ..Default::default()
            },
            None,
        );
        process_oai_chunk(&chunk, &mut state, &mut events, "test");

        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], AssistantMessageEvent::ThinkingDelta { content_index: 0, delta } if delta == " more")
        );
    }

    #[test]
    fn reasoning_to_content_transition_closes_thinking() {
        let mut state = OaiSseStreamState::default();
        let mut events = Vec::new();

        // Reasoning chunk
        let chunk = chunk_with_delta(
            OaiDelta {
                reasoning_content: Some("thinking...".to_string()),
                ..Default::default()
            },
            None,
        );
        process_oai_chunk(&chunk, &mut state, &mut events, "test");
        assert_eq!(events.len(), 2); // ThinkingStart + ThinkingDelta

        // Now regular content arrives → should close thinking, then open text
        events.clear();
        let chunk = chunk_with_delta(
            OaiDelta {
                content: Some("Hello".to_string()),
                ..Default::default()
            },
            None,
        );
        process_oai_chunk(&chunk, &mut state, &mut events, "test");

        // ThinkingEnd + TextStart + TextDelta
        assert_eq!(events.len(), 3);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                ..
            }
        ));
        assert!(matches!(
            &events[1],
            AssistantMessageEvent::TextStart { content_index: 1 }
        ));
        assert!(matches!(
            &events[2],
            AssistantMessageEvent::TextDelta { content_index: 1, delta } if delta == "Hello"
        ));
    }

    #[test]
    fn reasoning_to_tool_call_closes_thinking() {
        let mut state = OaiSseStreamState::default();
        let mut events = Vec::new();

        // Reasoning chunk
        let chunk = chunk_with_delta(
            OaiDelta {
                reasoning_content: Some("planning...".to_string()),
                ..Default::default()
            },
            None,
        );
        process_oai_chunk(&chunk, &mut state, &mut events, "test");
        events.clear();

        // Tool call arrives
        let chunk = chunk_with_delta(
            OaiDelta {
                tool_calls: Some(vec![OaiToolCallDelta {
                    index: 0,
                    id: Some("call_1".to_string()),
                    function: Some(OaiFunctionDelta {
                        name: Some("my_tool".to_string()),
                        arguments: Some(r#"{"a":1}"#.to_string()),
                    }),
                }]),
                ..Default::default()
            },
            None,
        );
        process_oai_chunk(&chunk, &mut state, &mut events, "test");

        // First event should be ThinkingEnd
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                ..
            }
        ));
        // Then ToolCallStart
        assert!(matches!(
            &events[1],
            AssistantMessageEvent::ToolCallStart {
                content_index: 1,
                ..
            }
        ));
    }

    #[test]
    fn chunks_without_reasoning_work_normally() {
        let mut state = OaiSseStreamState::default();
        let mut events = Vec::new();

        // Regular text chunk
        let chunk = chunk_with_delta(
            OaiDelta {
                content: Some("Hello world".to_string()),
                ..Default::default()
            },
            None,
        );
        process_oai_chunk(&chunk, &mut state, &mut events, "test");

        assert_eq!(events.len(), 2); // TextStart + TextDelta
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::TextStart { content_index: 0 }
        ));
        assert!(matches!(
            &events[1],
            AssistantMessageEvent::TextDelta { content_index: 0, delta } if delta == "Hello world"
        ));
    }

    #[test]
    fn empty_reasoning_content_ignored() {
        let mut state = OaiSseStreamState::default();
        let mut events = Vec::new();

        let chunk = chunk_with_delta(
            OaiDelta {
                reasoning_content: Some(String::new()),
                ..Default::default()
            },
            None,
        );
        process_oai_chunk(&chunk, &mut state, &mut events, "test");

        assert!(events.is_empty());
    }

    #[test]
    fn null_reasoning_content_ignored() {
        let mut state = OaiSseStreamState::default();
        let mut events = Vec::new();

        let chunk = chunk_with_delta(
            OaiDelta {
                reasoning_content: None,
                content: Some("text".to_string()),
                ..Default::default()
            },
            None,
        );
        process_oai_chunk(&chunk, &mut state, &mut events, "test");

        // Should just get text events, no thinking
        assert_eq!(events.len(), 2);
        assert!(matches!(
            &events[0],
            AssistantMessageEvent::TextStart { content_index: 0 }
        ));
    }

    #[test]
    fn reasoning_content_deserialized_from_json() {
        let json = r#"{
            "choices": [{
                "delta": {
                    "reasoning_content": "step by step"
                },
                "finish_reason": null
            }]
        }"#;

        let chunk: OaiChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(
            chunk.choices[0].delta.reasoning_content.as_deref(),
            Some("step by step")
        );
    }

    // ── Issue #619: incomplete tool_use sanitization ─────────────────────

    /// Regression for #619: after the loop-level scrub runs, an assistant
    /// message that originally carried `arguments: Null` with `partial_json`
    /// set must serialize with `function.arguments: "{}"` (a stringified empty
    /// object) rather than the literal string `"null"` that `Value::Null.to_string()`
    /// would produce. Otherwise OpenAI-compatible providers reject the request
    /// when they try to parse the arguments.
    #[test]
    fn assistant_message_sanitized_tool_call_serializes_empty_object_string() {
        use swink_agent::AssistantMessage;

        let mut assistant = AssistantMessage {
            content: vec![ContentBlock::ToolCall {
                id: "call_01".into(),
                name: "read_file".into(),
                arguments: Value::Null,
                partial_json: Some(r#"{"path": "/tm"#.into()),
            }],
            provider: "openai".into(),
            model_id: "gpt-4o-mini".into(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::Length,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        };

        swink_agent::sanitize_incomplete_tool_calls(&mut assistant);

        let oai_msg = OaiConverter::assistant_message(&assistant);
        assert_eq!(oai_msg.role, "assistant");
        let tool_calls = oai_msg.tool_calls.expect("tool_calls must be Some");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "read_file");
        // Critical: the stringified arguments must be a valid JSON object, NOT
        // the literal "null" that `Value::Null.to_string()` would produce.
        assert_eq!(tool_calls[0].function.arguments, "{}");
    }

    #[test]
    fn terminal_parse_error_flushes_pending_tool_call_before_generic_error() {
        let mut state = OaiSseStreamState::default();
        state.tool_calls.insert(
            0,
            OaiToolCallEntry {
                id: "call_1".into(),
                name: Some("read_file".into()),
                arguments: r#"{"path":"foo.rs"}"#.into(),
                content_index: None,
            },
        );

        let events = state.emit_terminal_error(AssistantMessageEvent::error(
            "OpenAI JSON parse error: bad payload",
        ));
        let delta_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::ToolCallDelta { .. }))
            .expect("final tool delta");
        let end_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::ToolCallEnd { .. }))
            .expect("tool call end");
        let error_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::Error { .. }))
            .expect("terminal error");

        assert!(
            delta_index < end_index && end_index < error_index,
            "pending tool-call state must flush before the terminal error: {events:?}"
        );
        match &events[error_index] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert!(
                    error_kind.is_none(),
                    "JSON parse errors must be non-retryable protocol errors"
                );
            }
            event => panic!("expected terminal error, got {event:?}"),
        }
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
            "terminal error path must not emit Done"
        );
    }

    #[test]
    fn unexpected_eof_flushes_pending_tool_call_before_network_error() {
        let mut state = OaiSseStreamState::default();
        state.tool_calls.insert(
            0,
            OaiToolCallEntry {
                id: "call_1".into(),
                name: Some("read_file".into()),
                arguments: r#"{"path":"foo.rs"}"#.into(),
                content_index: None,
            },
        );

        let events = state.emit_terminal_error(AssistantMessageEvent::error_network(
            "OpenAI-compatible stream ended unexpectedly",
        ));
        let delta_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::ToolCallDelta { .. }))
            .expect("final tool delta");
        let end_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::ToolCallEnd { .. }))
            .expect("tool call end");
        let error_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::Error { .. }))
            .expect("error event");

        assert!(
            delta_index < end_index && end_index < error_index,
            "pending tool-call state must flush before Error on unexpected EOF: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
            "transport EOF without [DONE] must not complete normally"
        );
        match &events[error_index] {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(*error_kind, Some(swink_agent::StreamErrorKind::Network));
            }
            event => panic!("expected network error, got {event:?}"),
        }
    }

    #[test]
    fn done_sentinel_preserves_accumulated_stop_reason() {
        let mut state = OaiSseStreamState {
            stop_reason: Some(StopReason::ToolUse),
            ..Default::default()
        };
        state.tool_calls.insert(
            0,
            OaiToolCallEntry {
                id: "call_1".into(),
                name: Some("read_file".into()),
                arguments: r#"{"path":"foo.rs"}"#.into(),
                content_index: None,
            },
        );

        let events = state.emit_done_from_done_sentinel();
        let delta_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::ToolCallDelta { .. }))
            .expect("final tool delta");
        let end_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::ToolCallEnd { .. }))
            .expect("tool call end");
        let done_index = events
            .iter()
            .position(|event| matches!(event, AssistantMessageEvent::Done { .. }))
            .expect("done event");

        assert!(
            delta_index < end_index && end_index < done_index,
            "pending tool-call state must flush before [DONE] completion: {events:?}"
        );
        match &events[done_index] {
            AssistantMessageEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::ToolUse);
            }
            event => panic!("expected done event, got {event:?}"),
        }
    }

    #[test]
    fn done_sentinel_reports_protocol_error_for_pending_tool_call_without_name() {
        let mut state = OaiSseStreamState {
            stop_reason: Some(StopReason::ToolUse),
            ..Default::default()
        };
        state.tool_calls.insert(
            0,
            OaiToolCallEntry {
                id: "call_1".into(),
                name: None,
                arguments: r#"{"path":"foo.rs"}"#.into(),
                content_index: None,
            },
        );

        let events = state.emit_done_from_done_sentinel();

        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AssistantMessageEvent::ToolCallStart { .. })),
            "terminal drain must not synthesize nameless ToolCallStart events: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, AssistantMessageEvent::Done { .. })),
            "nameless terminal tool calls must fail the stream instead of completing normally: {events:?}"
        );
        match events.last() {
            Some(AssistantMessageEvent::Error {
                error_message,
                error_kind,
                ..
            }) => {
                assert!(
                    error_kind.is_none(),
                    "protocol errors must not be retryable"
                );
                assert!(
                    error_message.contains("missing function name"),
                    "error should explain the terminal protocol fault: {error_message}"
                );
            }
            other => panic!("expected terminal protocol error, got {other:?}"),
        }
    }
}
