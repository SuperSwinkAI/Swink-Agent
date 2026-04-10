//! Ollama LLM adapter.
//!
//! Implements [`StreamFn`] for the Ollama `/api/chat` endpoint.
//! Ollama streams newline-delimited JSON (NDJSON), not SSE.

use std::collections::HashMap;
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
    ModelSpec, StopReason, StreamFn, StreamOptions, ThinkingLevel, ToolResultMessage, Usage,
    UserMessage,
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
            let code = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status = code, "Ollama HTTP error");
            let event = crate::classify::error_event_from_status(code, &body, "Ollama");
            return stream::iter(vec![event]).left_stream();
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
) -> impl Stream<Item = AssistantMessageEvent> + Send {
    let byte_stream = response.bytes_stream();
    let line_stream = ndjson_lines(byte_stream);

    stream::unfold(
        (Box::pin(line_stream), cancellation_token, StreamState {
            blocks: crate::block_accumulator::BlockAccumulator::new(),
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
                            events.push(AssistantMessageEvent::error_network("Ollama stream ended unexpectedly"));
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(Err(err)) => {
                            // Transport-level failure — surface as network error
                            // instead of silently treating as EOF.
                            error!(error = %err, "Ollama transport error");
                            done = true;
                            let mut events = crate::finalize::finalize_blocks(&mut state);
                            events.push(AssistantMessageEvent::error_network(format!(
                                "Ollama {err}"
                            )));
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(Ok(line)) => {
                            let chunk: OllamaChatChunk = match serde_json::from_str(&line) {
                                Ok(c) => c,
                                Err(e) => {
                                    error!(error = %e, "Ollama JSON parse error");
                                    done = true;
                                    let mut events = crate::finalize::finalize_blocks(&mut state);
                                    events.push(AssistantMessageEvent::error_network(format!("Ollama JSON parse error: {e}")));
                                    return Some((events, (lines, token, state, done, false)));
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
                                if let Some(ev) =
                                    state.blocks.text_delta(chunk.message.content.clone())
                                {
                                    events.push(ev);
                                }
                            }

                            // Handle tool calls
                            if let Some(tool_calls) = &chunk.message.tool_calls {
                                events.extend(emit_tool_calls(&mut state, tool_calls));
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
        crate::finalize::StreamFinalize::drain_open_blocks(&mut self.blocks)
    }
}

/// State machine tracking which content blocks have been started.
struct StreamState {
    blocks: crate::block_accumulator::BlockAccumulator,
}

/// Convert a byte stream into a stream of complete NDJSON lines.
///
/// Yields `Ok(line)` for each complete line and `Err(message)` if the
/// underlying `reqwest` byte stream reports a transport-level failure.
/// A transport error is terminal — downstream consumers must stop after
/// observing one.
fn ndjson_lines(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> Pin<Box<dyn Stream<Item = Result<String, String>> + Send + 'static>> {
    Box::pin(stream::unfold(
        (Box::pin(byte_stream), String::new(), false),
        |(mut stream, mut buf, mut errored)| async move {
            if errored {
                return None;
            }
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
                        return Some((Ok(line), (stream, buf, errored)));
                    }
                    continue;
                }

                // Need more data
                match stream.next().await {
                    Some(Ok(bytes)) => {
                        // Attempt zero-copy UTF-8 conversion
                        match std::str::from_utf8(&bytes) {
                            Ok(s) => buf.push_str(s),
                            Err(_) => buf.push_str(&String::from_utf8_lossy(&bytes)),
                        }
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
                        // Stream ended cleanly — flush remaining buffer
                        let trimmed = buf.trim();
                        if !trimmed.is_empty() {
                            let line = trimmed.to_string();
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
mod tests {
    use super::*;
    use crate::convert::convert_messages;
    use crate::finalize::StreamFinalize;
    use futures::StreamExt;
    use futures::stream;
    use swink_agent::{
        AgentMessage, AssistantMessage as HarnessAssistantMessage, ContentBlock, Cost, LlmMessage,
        StopReason, ToolResultMessage, Usage, UserMessage,
    };

    // ─── convert_messages: user + system ────────────────────────────────

    #[test]
    fn convert_user_and_system_messages() {
        let messages = vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))];

        let result = convert_messages::<OllamaConverter>(&messages, "test sys");

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "system");
        assert_eq!(result[0].content, "test sys");
        assert_eq!(result[1].role, "user");
        assert_eq!(result[1].content, "hello");
    }

    // ─── ndjson_lines ───────────────────────────────────────────────────

    #[tokio::test]
    async fn ndjson_splits_two_lines() {
        let bytes_stream = stream::iter(vec![Ok(bytes::Bytes::from("line1\nline2\n"))]);
        let mut lines = ndjson_lines(bytes_stream);

        assert_eq!(lines.next().await.unwrap().unwrap(), "line1");
        assert_eq!(lines.next().await.unwrap().unwrap(), "line2");
        assert!(lines.next().await.is_none());
    }

    #[tokio::test]
    async fn ndjson_crlf_line_endings() {
        let bytes_stream = stream::iter(vec![Ok(bytes::Bytes::from("aaa\r\nbbb\r\n"))]);
        let mut lines = ndjson_lines(bytes_stream);

        assert_eq!(lines.next().await.unwrap().unwrap(), "aaa");
        assert_eq!(lines.next().await.unwrap().unwrap(), "bbb");
        assert!(lines.next().await.is_none());
    }

    #[tokio::test]
    async fn ndjson_partial_lines_across_chunks() {
        let bytes_stream = stream::iter(vec![
            Ok(bytes::Bytes::from("hel")),
            Ok(bytes::Bytes::from("lo\nwor")),
            Ok(bytes::Bytes::from("ld\n")),
        ]);
        let mut lines = ndjson_lines(bytes_stream);

        assert_eq!(lines.next().await.unwrap().unwrap(), "hello");
        assert_eq!(lines.next().await.unwrap().unwrap(), "world");
        assert!(lines.next().await.is_none());
    }

    #[tokio::test]
    async fn ndjson_flush_remaining_buffer_no_trailing_newline() {
        let bytes_stream = stream::iter(vec![Ok(bytes::Bytes::from("no_newline"))]);
        let mut lines = ndjson_lines(bytes_stream);

        assert_eq!(lines.next().await.unwrap().unwrap(), "no_newline");
        assert!(lines.next().await.is_none());
    }

    /// Regression for issue #230 — transport errors on Ollama's NDJSON byte
    /// stream must surface as a terminal `Err` rather than being silently
    /// dropped as clean EOF. Uses a TCP listener that closes mid-body.
    #[tokio::test]
    async fn ndjson_surfaces_transport_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let header = "HTTP/1.1 200 OK\r\n\
                    Content-Type: application/x-ndjson\r\n\
                    Transfer-Encoding: chunked\r\n\r\n\
                    10\r\n{\"partial\":true";
                let _ = sock.write_all(header.as_bytes()).await;
                drop(sock);
            }
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("connect");
        let mut lines = ndjson_lines(resp.bytes_stream());

        let mut saw_err = false;
        while let Some(item) = lines.next().await {
            if item.is_err() {
                saw_err = true;
                break;
            }
        }
        assert!(saw_err, "expected Err from ndjson_lines on transport error");
    }

    #[tokio::test]
    async fn ndjson_empty_lines_skipped() {
        let bytes_stream = stream::iter(vec![Ok(bytes::Bytes::from("a\n\n\nb\n"))]);
        let mut lines = ndjson_lines(bytes_stream);

        assert_eq!(lines.next().await.unwrap().unwrap(), "a");
        assert_eq!(lines.next().await.unwrap().unwrap(), "b");
        assert!(lines.next().await.is_none());
    }

    // ─── StreamState drain_open_blocks ──────────────────────────────────

    #[test]
    fn drain_open_blocks_thinking_then_text() {
        let mut blocks = crate::block_accumulator::BlockAccumulator::new();
        blocks.ensure_thinking_open(); // index 0
        blocks.ensure_text_open(); // index 1
        let mut state = StreamState { blocks };

        let drained = state.drain_open_blocks();
        assert_eq!(drained.len(), 2);

        // Thinking comes first (content_index 0), then text (content_index 1)
        match &drained[0] {
            crate::finalize::OpenBlock::Thinking { content_index, .. } => {
                assert_eq!(*content_index, 0);
            }
            other => panic!("expected Thinking, got {other:?}"),
        }
        match &drained[1] {
            crate::finalize::OpenBlock::Text { content_index } => {
                assert_eq!(*content_index, 1);
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn drain_open_blocks_idempotent() {
        let mut blocks = crate::block_accumulator::BlockAccumulator::new();
        blocks.ensure_thinking_open();
        blocks.ensure_text_open();
        let mut state = StreamState { blocks };

        let first = state.drain_open_blocks();
        let second = state.drain_open_blocks();
        assert_eq!(first.len(), 2);
        assert!(second.is_empty());
    }

    // ─── convert_messages: assistant with tool calls ────────────────────

    #[test]
    fn convert_assistant_with_tool_calls() {
        let messages = vec![AgentMessage::Llm(LlmMessage::Assistant(
            HarnessAssistantMessage {
                content: vec![ContentBlock::ToolCall {
                    id: "tc_1".to_string(),
                    name: "my_tool".to_string(),
                    arguments: serde_json::json!({"key": "val"}),
                    partial_json: None,
                }],
                provider: "ollama".to_string(),
                model_id: "test".to_string(),
                usage: Usage::default(),
                cost: Cost::default(),
                stop_reason: StopReason::ToolUse,
                error_message: None,
                error_kind: None,
                timestamp: 0,
                cache_hint: None,
            },
        ))];

        let result = convert_messages::<OllamaConverter>(&messages, "");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "assistant");
        let tool_calls = result[0]
            .tool_calls
            .as_ref()
            .expect("should have tool_calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "my_tool");
        assert_eq!(
            tool_calls[0].function.arguments,
            serde_json::json!({"key": "val"})
        );
    }

    // ─── convert_messages: tool result ──────────────────────────────────

    #[test]
    fn convert_tool_result_message() {
        let messages = vec![AgentMessage::Llm(LlmMessage::ToolResult(
            ToolResultMessage {
                tool_call_id: "tc_1".to_string(),
                content: vec![ContentBlock::Text {
                    text: "result text".to_string(),
                }],
                is_error: false,
                timestamp: 0,
                details: serde_json::Value::Null,
                cache_hint: None,
            },
        ))];

        let result = convert_messages::<OllamaConverter>(&messages, "");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "tool");
        assert_eq!(result[0].content, "result text");
    }

    // ─── convert_messages: skips CustomMessage ──────────────────────────

    #[test]
    fn convert_skips_custom_message() {
        #[derive(Debug)]
        struct TestCustom;
        impl swink_agent::CustomMessage for TestCustom {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }

        let messages = vec![
            AgentMessage::Custom(Box::new(TestCustom)),
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "after custom".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            })),
        ];

        let result = convert_messages::<OllamaConverter>(&messages, "");

        // Only the user message should be present; custom is skipped.
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[0].content, "after custom");
    }

    // ─── emit_tool_calls: regression for issue #209 ─────────────────────

    /// Regression for issue #209: when Ollama emits two tool calls with the
    /// same function name in one chunk, both must be dispatched. The previous
    /// implementation deduped by name via a `HashSet<String>` and silently
    /// dropped the second call.
    #[test]
    fn emit_tool_calls_preserves_repeated_same_name_calls() {
        let mut state = StreamState {
            blocks: crate::block_accumulator::BlockAccumulator::new(),
        };

        let tool_calls = vec![
            OllamaResponseToolCall {
                function: OllamaResponseFunction {
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "a.txt"}),
                },
            },
            OllamaResponseToolCall {
                function: OllamaResponseFunction {
                    name: "read_file".to_string(),
                    arguments: serde_json::json!({"path": "b.txt"}),
                },
            },
        ];

        let events = emit_tool_calls(&mut state, &tool_calls);

        // Expect 6 events: Start/Delta/End for each of the two calls.
        let starts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                AssistantMessageEvent::ToolCallStart { name, id, .. } => Some((name, id)),
                _ => None,
            })
            .collect();
        assert_eq!(
            starts.len(),
            2,
            "both tool calls should produce a ToolCallStart"
        );
        assert_eq!(starts[0].0, "read_file");
        assert_eq!(starts[1].0, "read_file");
        assert_ne!(starts[0].1, starts[1].1, "tool call ids must be unique");

        let deltas: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                AssistantMessageEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas.len(), 2);
        assert!(deltas[0].contains("a.txt"));
        assert!(deltas[1].contains("b.txt"));

        let ends = events
            .iter()
            .filter(|e| matches!(e, AssistantMessageEvent::ToolCallEnd { .. }))
            .count();
        assert_eq!(ends, 2);
    }
}
