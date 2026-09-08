//! Ollama LLM adapter.
//!
//! Implements [`StreamFn`] for the Ollama `/api/chat` endpoint.
//! Ollama streams newline-delimited JSON (NDJSON), not SSE.

use std::pin::Pin;

use futures::stream::{self, Stream, StreamExt as _};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use swink_agent::ContentBlock;
use swink_agent::{
    AgentContext, AssistantMessage as HarnessAssistantMessage, AssistantMessageEvent, Cost,
    ModelSpec, ResponseFormat, StopReason, StreamFn, StreamOptions, ThinkingLevel,
    ToolResultMessage, Usage, UserMessage,
};

use crate::convert::{self, MessageConverter, extract_tool_schemas};
use crate::sse::{SseAction, sse_adapter_stream};

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
    options: Option<serde_json::Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<String>,
    /// Structured-output constraint. Top-level like `keep_alive`, *not* an
    /// `options.*` entry — Ollama ignores `options.format`.
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OllamaTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    think: Option<bool>,
}

/// Map [`ResponseFormat`] onto Ollama's top-level `format` field.
///
/// `Json` becomes the literal string `"json"`; `Schema` passes the JSON Schema
/// through verbatim, which is exactly what Ollama expects.
fn response_format(options: &StreamOptions) -> Option<Value> {
    options
        .serving
        .format
        .as_ref()
        .and_then(|format| match format {
            ResponseFormat::Json => Some(Value::from("json")),
            ResponseFormat::Schema(schema) => Some(schema.clone()),
            // Unknown future variant: we don't know how to represent it on the
            // wire, so omit `format` entirely rather than sending something wrong.
            _ => None,
        })
}

/// Build Ollama's `options` object from the stream/serving options.
///
/// `ServingOptions::extra` merges in via [`crate::base::merge_extra`] so typed
/// fields win on collision. Only the typed keys that are actually *set* are
/// protected: an unset typed knob never reaches the wire, so a same-named
/// `extra` entry passes through instead of colliding. Returns `None` when
/// nothing is set, so default requests carry no `options` key.
fn generation_options(options: &StreamOptions) -> Option<serde_json::Map<String, Value>> {
    let serving = &options.serving;
    let typed = [
        ("temperature", options.temperature.map(Value::from)),
        ("num_predict", options.max_tokens.map(Value::from)),
        ("top_p", serving.top_p.map(Value::from)),
        ("num_ctx", serving.context_length.map(Value::from)),
    ];
    let set_keys: Vec<&str> = typed
        .iter()
        .filter(|(_, value)| value.is_some())
        .map(|(key, _)| *key)
        .collect();

    let mut map = serde_json::Map::new();
    crate::base::merge_extra(&mut map, &serving.extra, &set_keys);
    for (key, value) in typed {
        if let Some(value) = value {
            map.insert(key.to_string(), value);
        }
    }
    (!map.is_empty()).then_some(map)
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
            base_url: base_url.into().trim_end_matches('/').to_string(),
            // Local inference: model cold-load into VRAM (or a huge-prompt
            // prefill) can sit silent well past the default 120s idle read
            // timeout before the first streamed byte — see the regression
            // caveat on issue #920. Use the generous local client instead.
            client: crate::base::local_adapter_http_client(),
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
    // Every serving field maps onto the Ollama request shape; stated
    // explicitly so this stays true even if the trait default changes.
    fn supported_serving_options(&self) -> swink_agent::ServingOptionSupport {
        swink_agent::ServingOptionSupport::all()
    }

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
        let response = match crate::base::race_pre_stream_cancellation(
            &cancellation_token,
            "Ollama request cancelled",
            send_request(ollama, model, context, options),
        )
        .await
        {
            Ok(resp) => resp,
            Err(event) => return stream::iter(crate::base::pre_stream_error(event)).left_stream(),
        };
        crate::base::report_rate_limit(response.headers(), options.on_rate_limit.as_ref());

        if !response.status().is_success() {
            let code = response.status().as_u16();
            let body = match crate::base::read_error_body_or_cancelled(
                response,
                &cancellation_token,
                "Ollama request cancelled",
            )
            .await
            {
                Ok(body) => body,
                Err(event) => {
                    return stream::iter(crate::base::pre_stream_error(event)).left_stream();
                }
            };
            warn!(status = code, "Ollama HTTP error");
            let event = crate::classify::error_event_from_status(code, &body, "Ollama");
            return stream::iter(crate::base::pre_stream_error(event)).left_stream();
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
        options: generation_options(options),
        keep_alive: options.serving.keep_alive.clone(),
        format: response_format(options),
        tools,
        think: if model.thinking_level == ThinkingLevel::Off {
            None
        } else {
            Some(true)
        },
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

/// Emit `ToolCallStart`/`Delta`/`End` events for each tool call in a chunk.
///
/// Ollama may legitimately emit the same tool name multiple times in one turn
/// (e.g. parallel calls to `read_file`). Each call must produce its own block —
/// deduplicating by name silently drops repeats. See issue #209.
fn emit_tool_calls(
    state: &mut StreamState,
    tool_calls: &[OllamaResponseToolCall],
) -> Vec<AssistantMessageEvent> {
    let mut events = Vec::with_capacity(tool_calls.len() * 3 + 1);

    // Close text block if open — tool calls always start a fresh block.
    if let Some(ev) = state.blocks.close_text() {
        events.push(ev);
    }

    for tc in tool_calls {
        let tool_id = format!("tc_{}", uuid::Uuid::new_v4());
        let (ci, start_ev) = state
            .blocks
            .open_tool_call(tool_id, tc.function.name.clone());
        events.push(start_ev);
        events.push(crate::block_accumulator::BlockAccumulator::tool_call_delta(
            ci,
            tc.function.arguments.to_string(),
        ));
        if let Some(ev) = state.blocks.close_tool_call(ci) {
            events.push(ev);
        }
    }
    events
}

/// Parse Ollama's NDJSON streaming response into `AssistantMessageEvent` values.
#[allow(clippy::too_many_lines)]
fn parse_ndjson_stream(
    response: reqwest::Response,
    cancellation_token: CancellationToken,
) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>> {
    let byte_stream = response.bytes_stream();
    let line_stream = ndjson_lines(byte_stream);

    sse_adapter_stream(
        line_stream,
        cancellation_token,
        StreamState {
            blocks: crate::block_accumulator::BlockAccumulator::new(),
        },
        "Ollama request cancelled",
        |item, state| match item {
            None => {
                // Stream ended without done=true
                let mut events = crate::finalize::finalize_blocks(state);
                events.push(AssistantMessageEvent::error_network(
                    "Ollama stream ended unexpectedly",
                ));
                SseAction::Done(events)
            }
            Some(Err(err)) => {
                // Transport-level failure — surface as network error
                // instead of silently treating as EOF.
                error!(error = %err, "Ollama transport error");
                let mut events = crate::finalize::finalize_blocks(state);
                events.push(AssistantMessageEvent::error_network(format!(
                    "Ollama {err}"
                )));
                SseAction::Done(events)
            }
            Some(Ok(line)) => {
                let chunk: OllamaChatChunk = match serde_json::from_str(&line) {
                    Ok(c) => c,
                    Err(e) => {
                        error!(error = %e, "Ollama JSON parse error");
                        let mut events = crate::finalize::finalize_blocks(state);
                        events.push(AssistantMessageEvent::error(format!(
                            "Ollama JSON parse error: {e}"
                        )));
                        return SseAction::Done(events);
                    }
                };

                let mut events = Vec::new();

                // Handle thinking content
                if let Some(thinking) = &chunk.message.thinking
                    && !thinking.is_empty()
                {
                    if let Some(ev) = state.blocks.ensure_thinking_open() {
                        events.push(ev);
                    }
                    if let Some(ev) = state.blocks.thinking_delta(thinking.clone()) {
                        events.push(ev);
                    }
                }

                // Handle text content
                if !chunk.message.content.is_empty() {
                    // Close thinking block first if open (returns None when not open)
                    if let Some(ev) = state.blocks.close_thinking(None) {
                        events.push(ev);
                    }
                    if let Some(ev) = state.blocks.ensure_text_open() {
                        events.push(ev);
                    }
                    if let Some(ev) = state.blocks.text_delta(chunk.message.content.clone()) {
                        events.push(ev);
                    }
                }

                // Handle tool calls
                if let Some(tool_calls) = &chunk.message.tool_calls {
                    events.extend(emit_tool_calls(state, tool_calls));
                }

                // Handle done
                if chunk.done {
                    events.extend(crate::finalize::finalize_blocks(state));

                    let stop_reason = match chunk.done_reason.as_deref() {
                        Some("tool_calls") => StopReason::ToolUse,
                        Some("length") => StopReason::Length,
                        _ => StopReason::Stop,
                    };

                    let input_tokens = chunk.prompt_eval_count.unwrap_or(0);
                    let output_tokens = chunk.eval_count.unwrap_or(0);

                    events.push(AssistantMessageEvent::Done {
                        stop_reason,
                        usage: Usage::default()
                            .with_input(input_tokens)
                            .with_output(output_tokens)
                            .with_total(input_tokens + output_tokens),
                        // Ollama is free / local — no cost
                        cost: Cost::default(),
                    });

                    return SseAction::Done(events);
                }

                if events.is_empty() {
                    // Skip empty chunks
                    SseAction::Skip
                } else {
                    SseAction::Continue(events)
                }
            }
        },
    )
}

impl crate::finalize::StreamFinalize for StreamState {
    fn drain_open_blocks(&mut self) -> Vec<crate::finalize::OpenBlock> {
        crate::finalize::StreamFinalize::drain_open_blocks(&mut self.blocks)
    }
}

/// State machine tracking which content blocks have been started.
struct StreamState {
    blocks: crate::block_accumulator::BlockAccumulator,
}

/// Convert a byte stream into a stream of complete NDJSON lines.
///
/// Yields `Ok(line)` for each complete line and `Err(message)` for any
/// terminal transport or UTF-8 decoding failure.
fn ndjson_lines(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> Pin<Box<dyn Stream<Item = Result<String, String>> + Send + 'static>> {
    Box::pin(stream::unfold(
        (Box::pin(byte_stream), Vec::<u8>::new(), false),
        |(mut stream, mut buf, mut errored)| async move {
            if errored {
                return None;
            }
            loop {
                // Check if we have a complete line in the buffer
                if let Some(pos) = buf.iter().position(|&byte| byte == b'\n') {
                    let remainder = buf.split_off(pos + 1);
                    let mut line = std::mem::replace(&mut buf, remainder);

                    line.pop(); // trailing '\n'
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }

                    if line.is_empty() {
                        continue;
                    }

                    match String::from_utf8(line) {
                        Ok(line) => return Some((Ok(line), (stream, buf, errored))),
                        Err(err) => {
                            errored = true;
                            buf.clear();
                            return Some((
                                Err(format!("invalid UTF-8 in NDJSON line: {err}")),
                                (stream, buf, errored),
                            ));
                        }
                    }
                }

                // Need more data
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        buf.extend_from_slice(&bytes);
                    }
                    Some(Err(err)) => {
                        // Transport failure — surface immediately as a
                        // terminal error so the adapter can emit a
                        // classified network error instead of EOF.
                        errored = true;
                        buf.clear();
                        return Some((
                            Err(format!("transport error: {err}")),
                            (stream, buf, errored),
                        ));
                    }
                    None => {
                        // Stream ended cleanly. A syntactically complete final
                        // JSON frame is valid without a trailing newline; an
                        // incomplete one is a transport EOF, not a protocol
                        // parse error.
                        let trimmed = buf
                            .iter()
                            .position(|byte| !byte.is_ascii_whitespace())
                            .map_or(&[][..], |start| {
                                let end = buf
                                    .iter()
                                    .rposition(|byte| !byte.is_ascii_whitespace())
                                    .expect("start implies a non-whitespace byte")
                                    + 1;
                                &buf[start..end]
                            });
                        if !trimmed.is_empty() {
                            let line = match String::from_utf8(trimmed.to_vec()) {
                                Ok(line) => line,
                                Err(err) => {
                                    errored = true;
                                    buf.clear();
                                    return Some((
                                        Err(format!(
                                            "invalid UTF-8 in trailing NDJSON line: {err}"
                                        )),
                                        (stream, buf, errored),
                                    ));
                                }
                            };
                            if let Err(err) = serde_json::from_str::<serde_json::Value>(&line) {
                                errored = true;
                                buf.clear();
                                return Some((
                                    Err(format!("incomplete trailing NDJSON frame: {err}")),
                                    (stream, buf, errored),
                                ));
                            }
                            buf.clear();
                            return Some((Ok(line), (stream, buf, errored)));
                        }
                        return None;
                    }
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

#[cfg(test)]
#[path = "ollama_tests.rs"]
mod tests;
