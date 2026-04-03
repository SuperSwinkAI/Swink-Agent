//! Shared OpenAI-compatible request/response types.
//!
//! Azure, Mistral, xAI, and plain `OpenAI` all use structurally identical
//! message, tool, and streaming chunk types. This module defines them once
//! so every adapter can reuse them without copy-paste.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{self, Stream, StreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::error;

use swink_agent::AgentTool;
use swink_agent::ContentBlock;
use swink_agent::stream::AssistantMessageEvent;
use swink_agent::types::{
    AssistantMessage as HarnessAssistantMessage, Cost, StopReason, ToolResultMessage, Usage,
    UserMessage,
};

use crate::convert::{MessageConverter, extract_tool_schemas};
use crate::sse::{SseLine, sse_data_lines};

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
}

/// The delta portion of a streaming choice.
#[derive(Default, Deserialize)]
pub struct OaiDelta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<OaiToolCallDelta>>,
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
}

// ─── Tool call state tracking ───────────────────────────────────────────────

/// Tracks the accumulated state of a single tool call during streaming.
pub struct ToolCallState {
    pub arguments: String,
    pub started: bool,
    pub content_index: usize,
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
#[derive(Default)]
pub struct OaiSseStreamState {
    pub text_started: bool,
    pub content_index: usize,
    pub tool_calls: HashMap<usize, ToolCallState>,
    pub usage: Option<Usage>,
    /// Saved stop reason from `finish_reason`; emitted with `Done` on `[DONE]`.
    pub stop_reason: Option<StopReason>,
}

impl crate::finalize::StreamFinalize for OaiSseStreamState {
    fn drain_open_blocks(&mut self) -> Vec<crate::finalize::OpenBlock> {
        let mut blocks = Vec::new();

        if self.text_started {
            blocks.push(crate::finalize::OpenBlock::Text {
                content_index: self.content_index,
            });
            self.text_started = false;
            self.content_index += 1;
        }

        let mut indices: Vec<usize> = self.tool_calls.keys().copied().collect();
        indices.sort_unstable();
        for idx in indices {
            if let Some(tc) = self.tool_calls.remove(&idx)
                && tc.started
            {
                blocks.push(crate::finalize::OpenBlock::ToolCall {
                    content_index: tc.content_index,
                });
            }
        }

        blocks
    }
}

/// Process a single deserialized `OaiChunk`, updating state and emitting events.
///
/// This is the shared chunk-processing logic used by both `OpenAI` and Azure
/// adapters. The `provider` label is used for fallback tool-call IDs.
pub fn process_oai_chunk(
    chunk: &OaiChunk,
    state: &mut OaiSseStreamState,
    events: &mut Vec<AssistantMessageEvent>,
    provider: &str,
) {
    if let Some(u) = &chunk.usage {
        state.usage = Some(Usage {
            input: u.prompt_tokens,
            output: u.completion_tokens,
            cache_read: 0,
            cache_write: 0,
            total: u.prompt_tokens + u.completion_tokens,
            extra: HashMap::new(),
        });
    }

    for choice in &chunk.choices {
        if let Some(content) = &choice.delta.content
            && !content.is_empty()
        {
            if !state.text_started {
                events.push(AssistantMessageEvent::TextStart {
                    content_index: state.content_index,
                });
                state.text_started = true;
            }
            events.push(AssistantMessageEvent::TextDelta {
                content_index: state.content_index,
                delta: content.clone(),
            });
        }

        if let Some(tool_calls) = &choice.delta.tool_calls {
            if state.text_started {
                events.push(AssistantMessageEvent::TextEnd {
                    content_index: state.content_index,
                });
                state.text_started = false;
                state.content_index += 1;
            }

            for tc_delta in tool_calls {
                process_oai_tool_call_delta(tc_delta, state, events, provider);
            }
        }

        if let Some(reason) = &choice.finish_reason {
            if reason == "content_filter" {
                events.extend(crate::finalize::finalize_blocks(state));
                events.push(AssistantMessageEvent::error_content_filtered(format!(
                    "{provider} response stopped by content filter"
                )));
                return;
            }

            let stop_reason = match reason.as_str() {
                "tool_calls" => StopReason::ToolUse,
                "length" | "model_length" => StopReason::Length,
                _ => StopReason::Stop,
            };

            events.extend(crate::finalize::finalize_blocks(state));

            state.stop_reason = Some(stop_reason);
        }
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

    if let std::collections::hash_map::Entry::Vacant(entry) = state.tool_calls.entry(tc_index) {
        let id = tc_delta
            .id
            .clone()
            .unwrap_or_else(|| format!("{provider}-tool-{tc_index}"));
        let name = tc_delta
            .function
            .as_ref()
            .and_then(|f| f.name.clone())
            .unwrap_or_default();

        let content_index = state.content_index;
        entry.insert(ToolCallState {
            arguments: String::new(),
            started: true,
            content_index,
        });
        state.content_index += 1;

        events.push(AssistantMessageEvent::ToolCallStart {
            content_index,
            id,
            name,
        });

        if let Some(args) = tc_delta
            .function
            .as_ref()
            .and_then(|f| f.arguments.as_ref())
            && !args.is_empty()
        {
            let tc_state = state.tool_calls.get_mut(&tc_index).expect("just inserted");
            tc_state.arguments.push_str(args);
            events.push(AssistantMessageEvent::ToolCallDelta {
                content_index,
                delta: args.clone(),
            });
        }
    } else {
        let tc_state = state
            .tool_calls
            .get_mut(&tc_index)
            .expect("entry exists per condition");
        if let Some(args) = tc_delta
            .function
            .as_ref()
            .and_then(|f| f.arguments.as_ref())
            && !args.is_empty()
        {
            tc_state.arguments.push_str(args);
            events.push(AssistantMessageEvent::ToolCallDelta {
                content_index: tc_state.content_index,
                delta: args.clone(),
            });
        }
    }
}

/// Parse an OpenAI-compatible SSE streaming response into `AssistantMessageEvent`
/// values.
///
/// This is the shared SSE state machine used by `OpenAI`, Azure, and other
/// OAI-compatible adapters. The `provider` label is used in error messages
/// and fallback tool-call IDs.
#[allow(clippy::too_many_lines)]
pub fn parse_oai_sse_stream(
    response: reqwest::Response,
    cancellation_token: CancellationToken,
    provider: &'static str,
) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>> {
    let byte_stream = response.bytes_stream();
    let line_stream = sse_data_lines(byte_stream);

    Box::pin(
        stream::unfold(
            (
                Box::pin(line_stream),
                cancellation_token,
                OaiSseStreamState::default(),
                false,
                true,
            ),
            move |(mut lines, token, mut state, mut done, first)| async move {
                if done {
                    return None;
                }

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
                                done = true;
                                let mut events = crate::finalize::finalize_blocks(&mut state);
                                if let Some(stop_reason) = state.stop_reason.take() {
                                    let usage = state.usage.take();
                                    events.push(AssistantMessageEvent::Done {
                                        stop_reason,
                                        usage: usage.unwrap_or_default(),
                                        cost: Cost::default(),
                                    });
                                } else {
                                    events.push(AssistantMessageEvent::error(
                                        format!("{provider} stream ended unexpectedly"),
                                    ));
                                }
                                Some((events, (lines, token, state, done, false)))
                            }
                            Some(SseLine::Done) => {
                                done = true;
                                let mut events = crate::finalize::finalize_blocks(&mut state);
                                let stop_reason = state.stop_reason.take()
                                    .unwrap_or(StopReason::Stop);
                                let usage = state.usage.take();
                                events.push(AssistantMessageEvent::Done {
                                    stop_reason,
                                    usage: usage.unwrap_or_default(),
                                    cost: Cost::default(),
                                });
                                Some((events, (lines, token, state, done, false)))
                            }
                            Some(SseLine::Data(data)) => {
                                let chunk: OaiChunk = match serde_json::from_str(&data) {
                                    Ok(c) => c,
                                    Err(e) => {
                                        error!(error = %e, "{provider} JSON parse error");
                                        done = true;
                                        let mut events = crate::finalize::finalize_blocks(&mut state);
                                        events.push(AssistantMessageEvent::error(format!(
                                            "{provider} JSON parse error: {e}"
                                        )));
                                        return Some((events, (lines, token, state, done, false)));
                                    }
                                };

                                let mut events = Vec::new();
                                process_oai_chunk(&chunk, &mut state, &mut events, provider);

                                if events.is_empty() {
                                    Some((vec![], (lines, token, state, done, false)))
                                } else {
                                    Some((events, (lines, token, state, done, false)))
                                }
                            }
                            Some(_) => {
                                Some((vec![], (lines, token, state, done, false)))
                            }
                        }
                    }
                }
            },
        )
        .flat_map(stream::iter),
    )
}
