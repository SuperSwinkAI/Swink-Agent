//! Azure `OpenAI` / Azure AI Foundry adapter.
//!
//! This adapter targets Azure's OpenAI-v1-compatible chat completions surface.

use std::collections::HashMap;
use std::pin::Pin;

use futures::stream::{self, Stream, StreamExt as _};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{AgentContext, Cost, ModelSpec, StopReason, Usage};

use crate::base::AdapterBase;
use crate::convert;
use crate::openai_compat::{
    OaiChatRequest, OaiChunk, OaiConverter, OaiStreamOptions, OaiToolCallDelta, ToolCallState,
    build_oai_tools,
};
use crate::sse::{SseLine, sse_data_lines};

pub struct AzureStreamFn {
    base: AdapterBase,
}

impl AzureStreamFn {
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base: AdapterBase::new(base_url.into().trim_end_matches('/').to_string(), api_key),
        }
    }
}

impl std::fmt::Debug for AzureStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AzureStreamFn")
            .field("base_url", &self.base.base_url)
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
            let event = crate::classify::error_event_from_status(code, &body, "Azure");
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
    let url = format!("{}/chat/completions", azure.base.base_url);
    debug!(
        %url,
        model = %model.model_id,
        messages = context.messages.len(),
        "sending Azure request"
    );

    let messages =
        convert::convert_messages::<OaiConverter>(&context.messages, &context.system_prompt);

    let (tools, tool_choice) = build_oai_tools(&context.tools);

    let body = OaiChatRequest {
        model: model.model_id.clone(),
        messages,
        stream: true,
        stream_options: OaiStreamOptions {
            include_usage: true,
        },
        temperature: options.temperature,
        max_tokens: options.max_tokens,
        tools,
        tool_choice,
    };

    let api_key = options.api_key.as_deref().unwrap_or(&azure.base.api_key);

    azure
        .base
        .client
        .post(&url)
        .header("api-key", api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| AssistantMessageEvent::error_network(format!("Azure connection error: {e}")))
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
                    let mut events = crate::finalize::finalize_blocks(&mut state);
                    events.push(AssistantMessageEvent::error_network("Azure request cancelled"));
                    done = true;
                    Some((events, (lines, token, state, done, false)))
                }
                maybe_line = lines.next() => {
                    match maybe_line {
                        None => {
                            let mut events = crate::finalize::finalize_blocks(&mut state);
                            events.push(AssistantMessageEvent::error("Azure stream ended unexpectedly"));
                            done = true;
                            Some((events, (lines, token, state, done, false)))
                        }
                        Some(SseLine::Done) => {
                            let mut events = crate::finalize::finalize_blocks(&mut state);
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
                            let chunk: OaiChunk = match serde_json::from_str(&line) {
                                Ok(chunk) => chunk,
                                Err(e) => {
                                    error!(error = %e, "Azure JSON parse error");
                                    events.push(AssistantMessageEvent::error(format!("Azure JSON parse error: {e}")));
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
    chunk: OaiChunk,
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
    tc_delta: OaiToolCallDelta,
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

impl crate::finalize::StreamFinalize for SseStreamState {
    fn drain_open_blocks(&mut self) -> Vec<crate::finalize::OpenBlock> {
        let mut blocks = Vec::new();

        if self.text_started {
            blocks.push(crate::finalize::OpenBlock::Text {
                content_index: self.content_index,
            });
            self.text_started = false;
            self.content_index += 1;
        }

        let mut tool_calls: Vec<_> = self.tool_calls.values().collect();
        tool_calls.sort_by_key(|tool_call| tool_call.content_index);
        for tool_call in tool_calls {
            blocks.push(crate::finalize::OpenBlock::ToolCall {
                content_index: tool_call.content_index,
            });
        }
        self.tool_calls.clear();

        blocks
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AzureStreamFn>();
};
