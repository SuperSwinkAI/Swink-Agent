//! Ollama LLM adapter.
//!
//! Implements [`StreamFn`] for the Ollama `/api/chat` endpoint.
//! Ollama streams newline-delimited JSON (NDJSON), not SSE.

use std::collections::{HashMap, HashSet};
use std::pin::Pin;

use futures::stream::{self, Stream, StreamExt as _};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use swink_agent::ContentBlock;
use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{
    AgentContext, AssistantMessage as HarnessAssistantMessage, Cost, ModelSpec, StopReason,
    ToolResultMessage, Usage, UserMessage,
};

use crate::convert::{self, MessageConverter, extract_tool_schemas};

// ─── Request types ──────────────────────────────────────────────────────────

/// Message in Ollama's format.
#[derive(Debug, Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OllamaToolCall>>,
}

/// Tool call in Ollama's format.
#[derive(Debug, Serialize)]
struct OllamaToolCall {
    function: OllamaFunctionCall,
}

/// Function call details.
#[derive(Debug, Serialize)]
struct OllamaFunctionCall {
    name: String,
    arguments: Value,
}

/// Tool definition in Ollama's format.
#[derive(Debug, Serialize)]
struct OllamaTool {
    r#type: String,
    function: OllamaToolDef,
}

/// Tool function definition.
#[derive(Debug, Serialize)]
struct OllamaToolDef {
    name: String,
    description: String,
    parameters: Value,
}

/// Full request body for Ollama /api/chat.
#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OllamaTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
}

/// Ollama generation options.
#[derive(Debug, Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u64>,
}

// ─── Response types ─────────────────────────────────────────────────────────

/// A single NDJSON chunk from Ollama's streaming response.
#[derive(Deserialize)]
struct OllamaChatChunk {
    message: OllamaResponseMessage,
    done: bool,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
}

/// The message portion of each chunk.
#[derive(Deserialize)]
struct OllamaResponseMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OllamaResponseToolCall>>,
}

/// Tool call in the response.
#[derive(Deserialize)]
struct OllamaResponseToolCall {
    function: OllamaResponseFunction,
}

/// Function details in a response tool call.
#[derive(Deserialize)]
struct OllamaResponseFunction {
    name: String,
    arguments: Value,
}

// ─── OllamaStreamFn ────────────────────────────────────────────────────────

/// A [`StreamFn`] implementation for Ollama's `/api/chat` endpoint.
///
/// Connects to a local or remote Ollama instance and streams responses
/// as `AssistantMessageEvent` values.
pub struct OllamaStreamFn {
    base_url: String,
    client: Client,
}

impl OllamaStreamFn {
    /// Create a new Ollama stream function.
    ///
    /// # Arguments
    ///
    /// * `base_url` - Ollama server URL (e.g. `http://localhost:11434`).
    #[must_use]
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            client: Client::new(),
        }
    }
}

impl std::fmt::Debug for OllamaStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OllamaStreamFn")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl StreamFn for OllamaStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(ollama_stream(
            self,
            model,
            context,
            options,
            cancellation_token,
        ))
    }
}

// ─── Stream implementation ──────────────────────────────────────────────────

fn ollama_stream<'a>(
    ollama: &'a OllamaStreamFn,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        let response = match send_request(ollama, model, context, options).await {
            Ok(resp) => resp,
            Err(event) => return stream::iter(vec![event]).left_stream(),
        };

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status, "Ollama HTTP error");
            return stream::iter(vec![AssistantMessageEvent::error_network(format!(
                "Ollama HTTP {status}: {body}"
            ))])
            .left_stream();
        }

        parse_ndjson_stream(response, cancellation_token).right_stream()
    })
    .flatten()
}

/// Send the HTTP POST request to Ollama.
async fn send_request(
    ollama: &OllamaStreamFn,
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
) -> Result<reqwest::Response, AssistantMessageEvent> {
    let url = format!("{}/api/chat", ollama.base_url);
    debug!(
        %url,
        model = %model.model_id,
        messages = context.messages.len(),
        "sending Ollama request"
    );

    let messages =
        convert::convert_messages::<OllamaConverter>(&context.messages, &context.system_prompt);

    let tools: Vec<OllamaTool> = extract_tool_schemas(&context.tools)
        .into_iter()
        .map(|s| OllamaTool {
            r#type: "function".to_string(),
            function: OllamaToolDef {
                name: s.name,
                description: s.description,
                parameters: s.parameters,
            },
        })
        .collect();

    let body = OllamaChatRequest {
        model: model.model_id.clone(),
        messages,
        stream: true,
        options: Some(OllamaOptions {
            temperature: options.temperature,
            num_predict: options.max_tokens,
        }),
        tools,
        think: None,
    };

    ollama
        .client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| AssistantMessageEvent::error_network(format!("Ollama connection error: {e}")))
}

// ─── MessageConverter impl ──────────────────────────────────────────────────

/// Marker type for Ollama-specific message conversion.
struct OllamaConverter;

impl MessageConverter for OllamaConverter {
    type Message = OllamaMessage;

    fn system_message(system_prompt: &str) -> Option<OllamaMessage> {
        Some(OllamaMessage {
            role: "system".to_string(),
            content: system_prompt.to_string(),
            tool_calls: None,
        })
    }

    fn user_message(user: &UserMessage) -> OllamaMessage {
        let content = ContentBlock::extract_text(&user.content);
        OllamaMessage {
            role: "user".to_string(),
            content,
            tool_calls: None,
        }
    }

    fn assistant_message(assistant: &HarnessAssistantMessage) -> OllamaMessage {
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        for block in &assistant.content {
            match block {
                ContentBlock::Text { text } => {
                    content.push_str(text);
                }
                ContentBlock::ToolCall {
                    name, arguments, ..
                } => {
                    tool_calls.push(OllamaToolCall {
                        function: OllamaFunctionCall {
                            name: name.clone(),
                            arguments: arguments.clone(),
                        },
                    });
                }
                _ => {}
            }
        }
        OllamaMessage {
            role: "assistant".to_string(),
            content,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
        }
    }

    fn tool_result_message(result: &ToolResultMessage) -> OllamaMessage {
        let content = ContentBlock::extract_text(&result.content);
        OllamaMessage {
            role: "tool".to_string(),
            content,
            tool_calls: None,
        }
    }
}

/// Parse Ollama's NDJSON streaming response into `AssistantMessageEvent` values.
#[allow(clippy::too_many_lines)]
fn parse_ndjson_stream(
    response: reqwest::Response,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send {
    let byte_stream = response.bytes_stream();
    let line_stream = ndjson_lines(byte_stream);

    stream::unfold(
        (Box::pin(line_stream), cancellation_token, StreamState {
            text_started: false,
            thinking_started: false,
            content_index: 0,
            tool_calls_started: HashSet::new(),
        }, false, true),
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
                            // Stream ended without done=true
                            done = true;
                            let mut events = crate::finalize::finalize_blocks(&mut state);
                            events.push(AssistantMessageEvent::error("Ollama stream ended unexpectedly"));
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(line) => {
                            let chunk: OllamaChatChunk = match serde_json::from_str(&line) {
                                Ok(c) => c,
                                Err(e) => {
                                    error!(error = %e, "Ollama JSON parse error");
                                    done = true;
                                    let mut events = crate::finalize::finalize_blocks(&mut state);
                                    events.push(AssistantMessageEvent::error(format!("Ollama JSON parse error: {e}")));
                                    return Some((events, (lines, token, state, done, false)));
                                }
                            };

                            let mut events = Vec::new();

                            // Handle thinking content
                            if let Some(thinking) = &chunk.message.thinking
                                && !thinking.is_empty()
                            {
                                if !state.thinking_started {
                                    events.push(AssistantMessageEvent::ThinkingStart {
                                        content_index: state.content_index,
                                    });
                                    state.thinking_started = true;
                                }
                                events.push(AssistantMessageEvent::ThinkingDelta {
                                    content_index: state.content_index,
                                    delta: thinking.clone(),
                                });
                            }

                            // Handle text content
                            if !chunk.message.content.is_empty() {
                                if !state.text_started {
                                    // Close thinking block first if open
                                    if state.thinking_started {
                                        events.push(AssistantMessageEvent::ThinkingEnd {
                                            content_index: state.content_index,
                                            signature: None,
                                        });
                                        state.thinking_started = false;
                                        state.content_index += 1;
                                    }
                                    events.push(AssistantMessageEvent::TextStart {
                                        content_index: state.content_index,
                                    });
                                    state.text_started = true;
                                }
                                events.push(AssistantMessageEvent::TextDelta {
                                    content_index: state.content_index,
                                    delta: chunk.message.content.clone(),
                                });
                            }

                            // Handle tool calls
                            if let Some(tool_calls) = &chunk.message.tool_calls {
                                // Close text block if open
                                if state.text_started {
                                    events.push(AssistantMessageEvent::TextEnd {
                                        content_index: state.content_index,
                                    });
                                    state.text_started = false;
                                    state.content_index += 1;
                                }

                                for tc in tool_calls {
                                    let tool_id = format!("tc_{}", uuid::Uuid::new_v4());
                                    if state.tool_calls_started.insert(tc.function.name.clone()) {
                                        events.push(AssistantMessageEvent::ToolCallStart {
                                            content_index: state.content_index,
                                            id: tool_id,
                                            name: tc.function.name.clone(),
                                        });
                                        events.push(AssistantMessageEvent::ToolCallDelta {
                                            content_index: state.content_index,
                                            delta: tc.function.arguments.to_string(),
                                        });
                                        events.push(AssistantMessageEvent::ToolCallEnd {
                                            content_index: state.content_index,
                                        });
                                        state.content_index += 1;
                                    }
                                }
                            }

                            // Handle done
                            if chunk.done {
                                done = true;
                                events.extend(crate::finalize::finalize_blocks(&mut state));

                                let stop_reason = match chunk.done_reason.as_deref() {
                                    Some("tool_calls") => StopReason::ToolUse,
                                    Some("length") => StopReason::Length,
                                    _ => StopReason::Stop,
                                };

                                let input_tokens = chunk.prompt_eval_count.unwrap_or(0);
                                let output_tokens = chunk.eval_count.unwrap_or(0);

                                events.push(AssistantMessageEvent::Done {
                                    stop_reason,
                                    usage: Usage {
                                        input: input_tokens,
                                        output: output_tokens,
                                        cache_read: 0,
                                        cache_write: 0,
                                        total: input_tokens + output_tokens,
                                        extra: HashMap::new(),
                                    },
                                    // Ollama is free / local — no cost
                                    cost: Cost {
                                        input: 0.0,
                                        output: 0.0,
                                        cache_read: 0.0,
                                        cache_write: 0.0,
                                        total: 0.0,
                                        extra: HashMap::new(),
                                    },
                                });
                            }

                            if events.is_empty() {
                                // Skip empty chunks
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

impl crate::finalize::StreamFinalize for StreamState {
    fn drain_open_blocks(&mut self) -> Vec<crate::finalize::OpenBlock> {
        let mut blocks = Vec::new();
        if self.thinking_started {
            blocks.push(crate::finalize::OpenBlock::Thinking {
                content_index: self.content_index,
                signature: None,
            });
            self.thinking_started = false;
            self.content_index += 1;
        }
        if self.text_started {
            blocks.push(crate::finalize::OpenBlock::Text {
                content_index: self.content_index,
            });
            self.text_started = false;
            self.content_index += 1;
        }
        blocks
    }
}

/// State machine tracking which content blocks have been started.
struct StreamState {
    text_started: bool,
    thinking_started: bool,
    content_index: usize,
    tool_calls_started: HashSet<String>,
}

/// Convert a byte stream into a stream of complete NDJSON lines.
fn ndjson_lines(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> Pin<Box<dyn Stream<Item = String> + Send + 'static>> {
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
                    if !line.is_empty() {
                        return Some((line, (stream, buf)));
                    }
                    continue;
                }

                // Need more data
                if let Some(Ok(bytes)) = stream.next().await {
                    // Attempt zero-copy UTF-8 conversion
                    match std::str::from_utf8(&bytes) {
                        Ok(s) => buf.push_str(s),
                        Err(_) => buf.push_str(&String::from_utf8_lossy(&bytes)),
                    }
                } else {
                    // Stream ended — flush remaining buffer
                    let trimmed = buf.trim();
                    if !trimmed.is_empty() {
                        let line = trimmed.to_string();
                        buf.clear();
                        return Some((line, (stream, buf)));
                    }
                    return None;
                }
            }
        },
    ))
}

// ─── Compile-time assertions ────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OllamaStreamFn>();
};
