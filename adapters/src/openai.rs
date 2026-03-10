//! OpenAI-compatible LLM adapter.
//!
//! Implements [`StreamFn`] for any OpenAI-compatible chat completions API
//! (OpenAI, vLLM, LM Studio, Groq, Together, etc.). These all share the
//! same SSE streaming format.

use std::collections::HashMap;
use std::pin::Pin;

use futures::stream::{self, Stream, StreamExt as _};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use agent_harness::ContentBlock;
use agent_harness::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use agent_harness::types::{
    AgentContext, AgentMessage, Cost, LlmMessage, ModelSpec, StopReason, Usage,
};

// ─── Request types ──────────────────────────────────────────────────────────

/// Message in OpenAI's chat completions format.
#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCallRequest>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

/// Tool call in the request (assistant message replay).
#[derive(Serialize)]
struct OpenAiToolCallRequest {
    id: String,
    r#type: String,
    function: OpenAiFunctionCallRequest,
}

/// Function call details in a request tool call.
#[derive(Serialize)]
struct OpenAiFunctionCallRequest {
    name: String,
    arguments: String,
}

/// Tool definition in OpenAI's format.
#[derive(Serialize)]
struct OpenAiTool {
    r#type: String,
    function: OpenAiToolDef,
}

/// Tool function definition.
#[derive(Serialize)]
struct OpenAiToolDef {
    name: String,
    description: String,
    parameters: Value,
}

/// Stream options for the request.
#[derive(Serialize)]
struct OpenAiStreamOptions {
    include_usage: bool,
}

/// Full request body for OpenAI `/v1/chat/completions`.
#[derive(Serialize)]
struct OpenAiChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    stream: bool,
    stream_options: OpenAiStreamOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

// ─── Response types ─────────────────────────────────────────────────────────

/// A single SSE chunk from OpenAI's streaming response.
#[derive(Deserialize)]
struct OpenAiChunk {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

/// A choice in a streaming chunk.
#[derive(Deserialize)]
struct OpenAiChoice {
    #[serde(default)]
    delta: OpenAiDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// The delta portion of a streaming choice.
#[derive(Default, Deserialize)]
struct OpenAiDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

/// A tool call delta in a streaming response.
#[derive(Deserialize)]
struct OpenAiToolCallDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiFunctionDelta>,
}

/// Function delta in a tool call.
#[derive(Deserialize)]
struct OpenAiFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

/// Usage information in the response.
#[derive(Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

// ─── Tool call state tracking ───────────────────────────────────────────────

/// Tracks the accumulated state of a single tool call during streaming.
struct ToolCallState {
    arguments: String,
    started: bool,
    content_index: usize,
}

// ─── OpenAiStreamFn ─────────────────────────────────────────────────────────

/// A [`StreamFn`] implementation for OpenAI-compatible chat completions APIs.
///
/// Works with OpenAI, vLLM, LM Studio, Groq, Together, and any other provider
/// that implements the OpenAI chat completions SSE streaming format.
pub struct OpenAiStreamFn {
    base_url: String,
    api_key: String,
    client: Client,
}

impl OpenAiStreamFn {
    /// Create a new OpenAI-compatible stream function.
    ///
    /// # Arguments
    ///
    /// * `base_url` - API base URL (e.g. `https://api.openai.com`).
    /// * `api_key` - Bearer token for authentication.
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            client: Client::new(),
        }
    }
}

impl std::fmt::Debug for OpenAiStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiStreamFn")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl StreamFn for OpenAiStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(openai_stream(
            self,
            model,
            context,
            options,
            cancellation_token,
        ))
    }
}

// ─── Stream implementation ──────────────────────────────────────────────────

fn openai_stream<'a>(
    openai: &'a OpenAiStreamFn,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        let response = match send_request(openai, model, context, options).await {
            Ok(resp) => resp,
            Err(event) => return stream::iter(vec![event]).left_stream(),
        };

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            let msg = match code {
                401 | 403 => format!("OpenAI auth error (HTTP {code}): {body}"),
                429 => format!("OpenAI rate limit (HTTP 429): {body}"),
                500..=599 => format!("OpenAI server error (HTTP {code}): {body}"),
                _ => format!("OpenAI HTTP {code}: {body}"),
            };
            return stream::iter(vec![error_event(&msg)]).left_stream();
        }

        parse_sse_stream(response, cancellation_token).right_stream()
    })
    .flatten()
}

/// Send the HTTP POST request to the OpenAI-compatible endpoint.
async fn send_request(
    openai: &OpenAiStreamFn,
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
) -> Result<reqwest::Response, AssistantMessageEvent> {
    let url = format!("{}/v1/chat/completions", openai.base_url);

    let messages = convert_messages(&context.messages, &context.system_prompt);

    let tools: Vec<OpenAiTool> = context
        .tools
        .iter()
        .map(|t| OpenAiTool {
            r#type: "function".to_string(),
            function: OpenAiToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters_schema().clone(),
            },
        })
        .collect();

    let tool_choice = if tools.is_empty() {
        None
    } else {
        Some("auto".to_string())
    };

    let body = OpenAiChatRequest {
        model: model.model_id.clone(),
        messages,
        stream: true,
        stream_options: OpenAiStreamOptions {
            include_usage: true,
        },
        temperature: options.temperature,
        max_tokens: options.max_tokens,
        tools,
        tool_choice,
    };

    openai
        .client
        .post(&url)
        .header("Authorization", format!("Bearer {}", openai.api_key))
        .json(&body)
        .send()
        .await
        .map_err(|e| error_event(&format!("OpenAI connection error: {e}")))
}

/// Convert harness messages to OpenAI message format.
fn convert_messages(messages: &[AgentMessage], system_prompt: &str) -> Vec<OpenAiMessage> {
    let mut result = Vec::new();

    // System prompt as first message
    if !system_prompt.is_empty() {
        result.push(OpenAiMessage {
            role: "system".to_string(),
            content: Some(system_prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    for msg in messages {
        let AgentMessage::Llm(llm) = msg else {
            continue;
        };
        match llm {
            LlmMessage::User(user) => {
                let content = ContentBlock::extract_text(&user.content);
                result.push(OpenAiMessage {
                    role: "user".to_string(),
                    content: Some(content),
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
            LlmMessage::Assistant(assistant) => {
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
                            tool_calls.push(OpenAiToolCallRequest {
                                id: id.clone(),
                                r#type: "function".to_string(),
                                function: OpenAiFunctionCallRequest {
                                    name: name.clone(),
                                    arguments: arguments.to_string(),
                                },
                            });
                        }
                        _ => {}
                    }
                }
                result.push(OpenAiMessage {
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
                });
            }
            LlmMessage::ToolResult(tool_result) => {
                let content = ContentBlock::extract_text(&tool_result.content);
                result.push(OpenAiMessage {
                    role: "tool".to_string(),
                    content: Some(content),
                    tool_calls: None,
                    tool_call_id: Some(tool_result.tool_call_id.clone()),
                });
            }
        }
    }

    result
}

/// Parse OpenAI's SSE streaming response into `AssistantMessageEvent` values.
#[allow(clippy::too_many_lines)]
fn parse_sse_stream(
    response: reqwest::Response,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send {
    let byte_stream = response.bytes_stream();
    let line_stream = sse_data_lines(byte_stream);

    stream::unfold(
        (
            Box::pin(line_stream),
            cancellation_token,
            SseStreamState {
                text_started: false,
                content_index: 0,
                tool_calls: HashMap::new(),
                usage: None,
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
                    let mut events = finalize_blocks(&mut state);
                    events.push(AssistantMessageEvent::Error {
                        stop_reason: StopReason::Aborted,
                        error_message: "operation cancelled".to_string(),
                        usage: None,
                    });
                    done = true;
                    Some((events, (lines, token, state, done, false)))
                }
                item = lines.next() => {
                    match item {
                        None => {
                            // Stream ended without [DONE]
                            done = true;
                            let mut events = finalize_blocks(&mut state);
                            events.push(error_event("OpenAI stream ended unexpectedly"));
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(SseLine::Done) => {
                            done = true;
                            let mut events = finalize_blocks(&mut state);
                            // If we have usage but haven't emitted Done yet, emit it now
                            let usage = state.usage.take();
                            events.push(AssistantMessageEvent::Done {
                                stop_reason: StopReason::Stop,
                                usage: usage.unwrap_or_default(),
                                cost: Cost::default(),
                            });
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(SseLine::Data(data)) => {
                            let chunk: OpenAiChunk = match serde_json::from_str(&data) {
                                Ok(c) => c,
                                Err(e) => {
                                    done = true;
                                    let mut events = finalize_blocks(&mut state);
                                    events.push(error_event(&format!(
                                        "OpenAI JSON parse error: {e}"
                                    )));
                                    return Some((events, (lines, token, state, done, false)));
                                }
                            };

                            let mut events = Vec::new();

                            // Capture usage if present
                            if let Some(u) = &chunk.usage {
                                state.usage = Some(Usage {
                                    input: u.prompt_tokens,
                                    output: u.completion_tokens,
                                    cache_read: 0,
                                    cache_write: 0,
                                    total: u.prompt_tokens + u.completion_tokens,
                                });
                            }

                            // Process choices
                            for choice in &chunk.choices {
                                // Handle text content
                                if let Some(content) = &choice.delta.content {
                                    if !content.is_empty() {
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
                                }

                                // Handle tool call deltas
                                if let Some(tool_calls) = &choice.delta.tool_calls {
                                    // Close text block if open before tool calls
                                    if state.text_started {
                                        events.push(AssistantMessageEvent::TextEnd {
                                            content_index: state.content_index,
                                        });
                                        state.text_started = false;
                                        state.content_index += 1;
                                    }

                                    for tc_delta in tool_calls {
                                        process_tool_call_delta(
                                            tc_delta,
                                            &mut state,
                                            &mut events,
                                        );
                                    }
                                }

                                // Handle finish reason
                                if let Some(reason) = &choice.finish_reason {
                                    let stop_reason = match reason.as_str() {
                                        "tool_calls" => StopReason::ToolUse,
                                        "length" => StopReason::Length,
                                        // "stop" | "content_filter" | _
                                        _ => StopReason::Stop,
                                    };

                                    // Finalize all open blocks
                                    events.extend(finalize_blocks(&mut state));

                                    let usage = state.usage.take();
                                    events.push(AssistantMessageEvent::Done {
                                        stop_reason,
                                        usage: usage.unwrap_or_default(),
                                        cost: Cost::default(),
                                    });
                                    done = true;
                                }
                            }

                            if events.is_empty() {
                                Some((vec![], (lines, token, state, done, false)))
                            } else {
                                Some((events, (lines, token, state, done, false)))
                            }
                        }
                    }
                }
            }
        },
    )
    .flat_map(stream::iter)
}

/// Process a single tool call delta, updating state and emitting events.
fn process_tool_call_delta(
    tc_delta: &OpenAiToolCallDelta,
    state: &mut SseStreamState,
    events: &mut Vec<AssistantMessageEvent>,
) {
    let tc_index = tc_delta.index;

    if let std::collections::hash_map::Entry::Vacant(entry) = state.tool_calls.entry(tc_index) {
        // New tool call — create state
        let id = tc_delta
            .id
            .clone()
            .unwrap_or_else(|| format!("tc_{}", uuid::Uuid::new_v4()));
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

        // Append any initial arguments
        if let Some(args) = tc_delta.function.as_ref().and_then(|f| f.arguments.as_ref()) {
            if !args.is_empty() {
                let tc_state = state.tool_calls.get_mut(&tc_index).expect("just inserted");
                tc_state.arguments.push_str(args);
                events.push(AssistantMessageEvent::ToolCallDelta {
                    content_index,
                    delta: args.clone(),
                });
            }
        }
    } else {
        // Existing tool call — append arguments
        let tc_state = state
            .tool_calls
            .get_mut(&tc_index)
            .expect("entry exists per condition");
        if let Some(args) = tc_delta.function.as_ref().and_then(|f| f.arguments.as_ref()) {
            if !args.is_empty() {
                tc_state.arguments.push_str(args);
                events.push(AssistantMessageEvent::ToolCallDelta {
                    content_index: tc_state.content_index,
                    delta: args.clone(),
                });
            }
        }
    }
}

/// Close any open content blocks.
fn finalize_blocks(state: &mut SseStreamState) -> Vec<AssistantMessageEvent> {
    let mut events = Vec::new();

    if state.text_started {
        events.push(AssistantMessageEvent::TextEnd {
            content_index: state.content_index,
        });
        state.text_started = false;
        state.content_index += 1;
    }

    // Finalize all pending tool calls
    let mut indices: Vec<usize> = state.tool_calls.keys().copied().collect();
    indices.sort_unstable();
    for idx in indices {
        if let Some(tc) = state.tool_calls.remove(&idx) {
            if tc.started {
                events.push(AssistantMessageEvent::ToolCallEnd {
                    content_index: tc.content_index,
                });
            }
        }
    }

    events
}

/// State machine tracking SSE streaming progress.
struct SseStreamState {
    text_started: bool,
    content_index: usize,
    tool_calls: HashMap<usize, ToolCallState>,
    usage: Option<Usage>,
}

/// Parsed SSE line.
enum SseLine {
    /// A `data: [DONE]` signal.
    Done,
    /// A `data: {json}` payload.
    Data(String),
}

/// Convert a byte stream into a stream of parsed SSE data lines.
fn sse_data_lines(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> Pin<Box<dyn Stream<Item = SseLine> + Send + 'static>> {
    Box::pin(stream::unfold(
        (Box::pin(byte_stream), String::new()),
        |(mut stream, mut buf)| async move {
            loop {
                // Check if we have a complete line in the buffer
                if let Some(pos) = buf.find('\n') {
                    let line_end = if pos > 0 && buf.as_bytes().get(pos - 1) == Some(&b'\r') {
                        pos - 1
                    } else {
                        pos
                    };
                    let line: String = buf[..line_end].to_string();
                    buf.drain(..=pos);

                    // Skip empty lines (SSE event separators)
                    if line.is_empty() {
                        continue;
                    }

                    // Only process data lines
                    if let Some(data) = line.strip_prefix("data: ") {
                        let data = data.trim();
                        if data == "[DONE]" {
                            return Some((SseLine::Done, (stream, buf)));
                        }
                        if !data.is_empty() {
                            return Some((SseLine::Data(data.to_string()), (stream, buf)));
                        }
                    }
                    // Skip non-data lines (e.g. `event:`, `id:`, comments)
                    continue;
                }

                // Need more data
                if let Some(Ok(bytes)) = stream.next().await {
                    match std::str::from_utf8(&bytes) {
                        Ok(s) => buf.push_str(s),
                        Err(_) => buf.push_str(&String::from_utf8_lossy(&bytes)),
                    }
                } else {
                    // Stream ended — flush remaining buffer
                    let remaining = buf.trim().to_string();
                    buf.clear();
                    if !remaining.is_empty() {
                        if let Some(data) = remaining.strip_prefix("data: ") {
                            let data = data.trim();
                            if data == "[DONE]" {
                                return Some((SseLine::Done, (stream, buf)));
                            }
                            if !data.is_empty() {
                                return Some((SseLine::Data(data.to_string()), (stream, buf)));
                            }
                        }
                    }
                    return None;
                }
            }
        },
    ))
}

/// Create an error event.
fn error_event(message: &str) -> AssistantMessageEvent {
    AssistantMessageEvent::Error {
        stop_reason: StopReason::Error,
        error_message: message.to_string(),
        usage: None,
    }
}

// ─── Compile-time assertions ────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OpenAiStreamFn>();
};
