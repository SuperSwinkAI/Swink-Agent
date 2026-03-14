//! Azure `OpenAI` / Azure AI Foundry adapter.
//!
//! This adapter targets Azure's OpenAI-v1-compatible chat completions surface.

use std::collections::HashMap;
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

use crate::convert::{
    self, MessageConverter, error_event, error_event_auth, error_event_network,
    error_event_throttled, extract_tool_schemas,
};
use crate::sse::SseLine;

#[derive(Debug, Serialize)]
struct AzureMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<AzureToolCallRequest>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct AzureToolCallRequest {
    id: String,
    r#type: String,
    function: AzureFunctionCallRequest,
}

#[derive(Debug, Serialize)]
struct AzureFunctionCallRequest {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct AzureTool {
    r#type: String,
    function: AzureToolDef,
}

#[derive(Debug, Serialize)]
struct AzureToolDef {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Serialize)]
struct AzureStreamOptions {
    include_usage: bool,
}

#[derive(Debug, Serialize)]
struct AzureChatRequest {
    model: String,
    messages: Vec<AzureMessage>,
    stream: bool,
    stream_options: AzureStreamOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AzureTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

#[derive(Deserialize)]
struct AzureChunk {
    #[serde(default)]
    choices: Vec<AzureChoice>,
    #[serde(default)]
    usage: Option<AzureUsage>,
}

#[derive(Deserialize)]
struct AzureChoice {
    #[serde(default)]
    delta: AzureDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct AzureDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<AzureToolCallDelta>>,
}

#[derive(Deserialize)]
struct AzureToolCallDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<AzureFunctionDelta>,
}

#[derive(Deserialize)]
struct AzureFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct AzureUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

struct ToolCallState {
    arguments: String,
    started: bool,
    content_index: usize,
}

pub struct AzureStreamFn {
    base_url: String,
    api_key: String,
    client: Client,
}

impl AzureStreamFn {
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            client: Client::new(),
        }
    }
}

impl std::fmt::Debug for AzureStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureStreamFn")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl StreamFn for AzureStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(azure_stream(
            self,
            model,
            context,
            options,
            cancellation_token,
        ))
    }
}

fn azure_stream<'a>(
    azure: &'a AzureStreamFn,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        let response = match send_request(azure, model, context, options).await {
            Ok(resp) => resp,
            Err(event) => return stream::iter(vec![event]).left_stream(),
        };

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            let body = response.text().await.unwrap_or_default();
            warn!(status = code, "Azure HTTP error");
            let event = match code {
                401 | 403 => error_event_auth(&format!("Azure auth error (HTTP {code}): {body}")),
                429 => error_event_throttled(&format!("Azure rate limit (HTTP 429): {body}")),
                500..=599 => {
                    error_event_network(&format!("Azure server error (HTTP {code}): {body}"))
                }
                _ => error_event(&format!("Azure HTTP {code}: {body}")),
            };
            return stream::iter(vec![event]).left_stream();
        }

        parse_sse_stream(response, cancellation_token).right_stream()
    })
    .flatten()
}

async fn send_request(
    azure: &AzureStreamFn,
    model: &ModelSpec,
    context: &AgentContext,
    options: &StreamOptions,
) -> Result<reqwest::Response, AssistantMessageEvent> {
    let url = format!("{}/chat/completions", azure.base_url);
    debug!(
        %url,
        model = %model.model_id,
        messages = context.messages.len(),
        "sending Azure request"
    );

    let messages =
        convert::convert_messages::<AzureConverter>(&context.messages, &context.system_prompt);

    let tools: Vec<AzureTool> = extract_tool_schemas(&context.tools)
        .into_iter()
        .map(|schema| AzureTool {
            r#type: "function".to_string(),
            function: AzureToolDef {
                name: schema.name,
                description: schema.description,
                parameters: schema.parameters,
            },
        })
        .collect();

    let body = AzureChatRequest {
        model: model.model_id.clone(),
        messages,
        stream: true,
        stream_options: AzureStreamOptions {
            include_usage: true,
        },
        temperature: options.temperature,
        max_tokens: options.max_tokens,
        tool_choice: (!tools.is_empty()).then_some("auto".to_string()),
        tools,
    };

    let api_key = options.api_key.as_deref().unwrap_or(&azure.api_key);

    azure
        .client
        .post(&url)
        .header("api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| error_event_network(&format!("Azure connection error: {e}")))
}

struct AzureConverter;

impl MessageConverter for AzureConverter {
    type Message = AzureMessage;

    fn system_message(system_prompt: &str) -> Option<Self::Message> {
        Some(AzureMessage {
            role: "system".to_string(),
            content: Some(system_prompt.to_string()),
            tool_calls: None,
            tool_call_id: None,
        })
    }

    fn user_message(user: &UserMessage) -> Self::Message {
        AzureMessage {
            role: "user".to_string(),
            content: Some(ContentBlock::extract_text(&user.content)),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn assistant_message(assistant: &HarnessAssistantMessage) -> Self::Message {
        let mut content = String::new();
        let mut tool_calls = Vec::new();

        for block in &assistant.content {
            match block {
                ContentBlock::Text { text } => content.push_str(text),
                ContentBlock::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    tool_calls.push(AzureToolCallRequest {
                        id: id.clone(),
                        r#type: "function".to_string(),
                        function: AzureFunctionCallRequest {
                            name: name.clone(),
                            arguments: arguments.to_string(),
                        },
                    });
                }
                _ => {}
            }
        }

        AzureMessage {
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

    fn tool_result_message(result: &ToolResultMessage) -> Self::Message {
        AzureMessage {
            role: "tool".to_string(),
            content: Some(ContentBlock::extract_text(&result.content)),
            tool_calls: None,
            tool_call_id: Some(result.tool_call_id.clone()),
        }
    }
}

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
                stop_reason: None,
            },
            false,
            true,
        ),
        |(mut lines, token, mut state, mut done, first)| async move {
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
                    let mut events = finalize_blocks(&mut state);
                    events.push(AssistantMessageEvent::error_network("Azure request cancelled"));
                    done = true;
                    Some((events, (lines, token, state, done, false)))
                }
                maybe_line = lines.next() => {
                    match maybe_line {
                        None => {
                            let mut events = finalize_blocks(&mut state);
                            events.push(error_event("Azure stream ended unexpectedly"));
                            done = true;
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(SseLine::Done) => {
                            let mut events = finalize_blocks(&mut state);
                            events.push(AssistantMessageEvent::Done {
                                stop_reason: state.stop_reason.unwrap_or(StopReason::Stop),
                                usage: state.usage.clone().unwrap_or_default(),
                                cost: Cost::default(),
                            });
                            done = true;
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(SseLine::Data(line)) => {
                            let mut events = Vec::new();
                            let chunk: AzureChunk = match serde_json::from_str(&line) {
                                Ok(chunk) => chunk,
                                Err(e) => {
                                    error!(error = %e, "Azure JSON parse error");
                                    events.push(error_event(&format!("Azure JSON parse error: {e}")));
                                    done = true;
                                    return Some((events, (lines, token, state, done, false)));
                                }
                            };
                            process_chunk(chunk, &mut state, &mut events);
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(SseLine::Event(_) | SseLine::Empty) => {
                            Some((Vec::new(), (lines, token, state, done, false)))
                        }
                    }
                }
            }
        },
    )
    .flat_map(stream::iter)
}

#[derive(Default)]
struct SseStreamState {
    text_started: bool,
    content_index: usize,
    tool_calls: HashMap<usize, ToolCallState>,
    usage: Option<Usage>,
    stop_reason: Option<StopReason>,
}

fn process_chunk(
    chunk: AzureChunk,
    state: &mut SseStreamState,
    events: &mut Vec<AssistantMessageEvent>,
) {
    if let Some(usage) = chunk.usage {
        state.usage = Some(Usage {
            input: usage.prompt_tokens,
            output: usage.completion_tokens,
            total: usage.prompt_tokens + usage.completion_tokens,
            ..Usage::default()
        });
    }

    for choice in chunk.choices {
        if let Some(content) = choice.delta.content
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
                delta: content,
            });
        }

        if let Some(tool_calls) = choice.delta.tool_calls {
            if state.text_started {
                events.push(AssistantMessageEvent::TextEnd {
                    content_index: state.content_index,
                });
                state.text_started = false;
                state.content_index += 1;
            }

            for tool_call in tool_calls {
                process_tool_call(tool_call, state, events);
            }
        }

        if let Some(finish_reason) = choice.finish_reason.as_deref() {
            state.stop_reason = Some(match finish_reason {
                "tool_calls" => StopReason::ToolUse,
                "length" => StopReason::Length,
                _ => StopReason::Stop,
            });
        }
    }
}

fn process_tool_call(
    tc_delta: AzureToolCallDelta,
    state: &mut SseStreamState,
    events: &mut Vec<AssistantMessageEvent>,
) {
    let entry = state.tool_calls.entry(tc_delta.index).or_insert_with(|| {
        let content_index = state.content_index;
        state.content_index += 1;
        ToolCallState {
            arguments: String::new(),
            started: false,
            content_index,
        }
    });

    if !entry.started {
        entry.started = true;
        let id = tc_delta
            .id
            .clone()
            .unwrap_or_else(|| format!("azure-tool-{}", tc_delta.index));
        let name = tc_delta
            .function
            .as_ref()
            .and_then(|function| function.name.clone())
            .unwrap_or_else(|| "tool".to_string());
        events.push(AssistantMessageEvent::ToolCallStart {
            content_index: entry.content_index,
            id,
            name,
        });
    }

    if let Some(function) = tc_delta.function
        && let Some(arguments) = function.arguments
        && !arguments.is_empty()
    {
        entry.arguments.push_str(&arguments);
        events.push(AssistantMessageEvent::ToolCallDelta {
            content_index: entry.content_index,
            delta: arguments,
        });
    }
}

fn finalize_blocks(state: &mut SseStreamState) -> Vec<AssistantMessageEvent> {
    let mut events = Vec::new();

    if state.text_started {
        events.push(AssistantMessageEvent::TextEnd {
            content_index: state.content_index,
        });
        state.text_started = false;
        state.content_index += 1;
    }

    let mut tool_calls: Vec<_> = state.tool_calls.values().collect();
    tool_calls.sort_by_key(|tool_call| tool_call.content_index);
    for tool_call in tool_calls {
        events.push(AssistantMessageEvent::ToolCallEnd {
            content_index: tool_call.content_index,
        });
    }
    state.tool_calls.clear();

    events
}

fn sse_data_lines(
    byte_stream: impl Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
) -> Pin<Box<dyn Stream<Item = crate::sse::SseLine> + Send + 'static>> {
    Box::pin(stream::unfold(
        (Box::pin(byte_stream), String::new()),
        |(mut stream, mut buf)| async move {
            loop {
                if let Some(pos) = buf.find('\n') {
                    let line_end = if pos > 0 && buf.as_bytes().get(pos - 1) == Some(&b'\r') {
                        pos - 1
                    } else {
                        pos
                    };
                    let line = buf[..line_end].to_string();
                    buf.drain(..=pos);
                    if line.is_empty() {
                        continue;
                    }
                    if let Some(data) = line.strip_prefix("data: ") {
                        let data = data.trim();
                        if data == "[DONE]" {
                            return Some((crate::sse::SseLine::Done, (stream, buf)));
                        }
                        if !data.is_empty() {
                            return Some((
                                crate::sse::SseLine::Data(data.to_string()),
                                (stream, buf),
                            ));
                        }
                    }
                    continue;
                }

                if let Some(Ok(bytes)) = stream.next().await {
                    match std::str::from_utf8(&bytes) {
                        Ok(s) => buf.push_str(s),
                        Err(_) => buf.push_str(&String::from_utf8_lossy(&bytes)),
                    }
                } else {
                    let remaining = buf.trim().to_string();
                    buf.clear();
                    if !remaining.is_empty()
                        && let Some(data) = remaining.strip_prefix("data: ")
                    {
                        let data = data.trim();
                        if data == "[DONE]" {
                            return Some((crate::sse::SseLine::Done, (stream, buf)));
                        }
                        if !data.is_empty() {
                            return Some((
                                crate::sse::SseLine::Data(data.to_string()),
                                (stream, buf),
                            ));
                        }
                    }
                    return None;
                }
            }
        },
    ))
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AzureStreamFn>();
};
