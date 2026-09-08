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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    /// Structured-output constraint ([`ServingOptions::format`]), pre-wrapped
    /// in the protocol's `{"type": …}` envelope by the caller.
    ///
    /// [`ServingOptions::format`]: swink_agent::ServingOptions::format
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OaiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    /// Extra provider-native body fields ([`ServingOptions::extra`]); callers
    /// must exclude keys that collide with the typed fields above (build this
    /// with `crate::base::merge_extra`).
    ///
    /// [`ServingOptions::extra`]: swink_agent::ServingOptions
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
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

        Usage::default()
            .with_input(self.prompt_tokens)
            .with_output(self.completion_tokens)
            .with_total(
                self.total_tokens
                    .unwrap_or(self.prompt_tokens + self.completion_tokens),
            )
            .with_extra(extra)
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
    /// When true, a `finish_reason: "error"` chunk is surfaced as a terminal
    /// `AssistantMessageEvent::Error` instead of falling through to the
    /// generic `stop_reason` mapping. Set by adapters (e.g. Mistral) whose
    /// provider uses `"error"` as a genuine terminal-failure finish reason.
    pub(crate) error_finish_reason_is_error: bool,
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

            if options.error_finish_reason_is_error && reason == "error" {
                let _ = flush_pending_oai_tool_calls(state, events, provider);
                events.extend(crate::finalize::finalize_blocks(state));
                state.terminal_error = Some(AssistantMessageEvent::Error {
                    stop_reason: StopReason::Error,
                    error_message: format!("{provider} reported finish_reason=error"),
                    usage: state.usage.clone(),
                    error_kind: None,
                    retry_after: None,
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
#[path = "openai_compat_tests.rs"]
mod tests;
