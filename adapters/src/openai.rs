//! OpenAI-compatible LLM adapter.
//!
//! Implements [`StreamFn`] for any OpenAI-compatible chat completions API
//! (OpenAI, vLLM, LM Studio, Groq, Together, etc.). These all share the
//! same SSE streaming format.

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

// ─── OpenAiStreamFn ─────────────────────────────────────────────────────────

/// A [`StreamFn`] implementation for OpenAI-compatible chat completions APIs.
///
/// Works with OpenAI, vLLM, LM Studio, Groq, Together, and any other provider
/// that implements the OpenAI chat completions SSE streaming format.
pub struct OpenAiStreamFn {
    pub(crate) base: AdapterBase,
}

impl OpenAiStreamFn {
    /// Create a new OpenAI-compatible stream function.
    ///
    /// # Arguments
    ///
    /// * `base_url` - API base URL (e.g. `https://api.openai.com`).
    /// * `api_key` - Bearer token for authentication.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            base: AdapterBase::new(base_url, api_key),
        }
    }
}

impl std::fmt::Debug for OpenAiStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiStreamFn")
            .field("base_url", &self.base.base_url)
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
            warn!(status = code, "OpenAI HTTP error");
            let event = crate::classify::error_event_from_status(code, &body, "OpenAI");
            return stream::iter(vec![event]).left_stream();
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
    let url = format!("{}/v1/chat/completions", openai.base.base_url);
    debug!(
        %url,
        model = %model.model_id,
        messages = context.messages.len(),
        "sending OpenAI request"
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

    let api_key = options.api_key.as_deref().unwrap_or(&openai.base.api_key);

    openai
        .base
        .client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| AssistantMessageEvent::error_network(format!("OpenAI connection error: {e}")))
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
                            // Stream ended without [DONE]
                            done = true;
                            let mut events = crate::finalize::finalize_blocks(&mut state);
                            if let Some(stop_reason) = state.stop_reason.take() {
                                // Had a finish_reason but no [DONE] — still valid
                                let usage = state.usage.take();
                                events.push(AssistantMessageEvent::Done {
                                    stop_reason,
                                    usage: usage.unwrap_or_default(),
                                    cost: Cost::default(),
                                });
                            } else {
                                events.push(AssistantMessageEvent::error("OpenAI stream ended unexpectedly"));
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
                                    error!(error = %e, "OpenAI JSON parse error");
                                    done = true;
                                    let mut events = crate::finalize::finalize_blocks(&mut state);
                                    events.push(AssistantMessageEvent::error(format!(
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
                                    extra: HashMap::new(),
                                });
                            }

                            // Process choices
                            for choice in &chunk.choices {
                                // Handle text content
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

                                // Handle finish reason — save for [DONE]; usage may
                                // arrive in a subsequent chunk.
                                if let Some(reason) = &choice.finish_reason {
                                    let stop_reason = match reason.as_str() {
                                        "tool_calls" => StopReason::ToolUse,
                                        "length" => StopReason::Length,
                                        // "stop" | "content_filter" | _
                                        _ => StopReason::Stop,
                                    };

                                    // Finalize all open blocks
                                    events.extend(crate::finalize::finalize_blocks(&mut state));

                                    state.stop_reason = Some(stop_reason);
                                }
                            }

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
    .flat_map(stream::iter)
}

/// Process a single tool call delta, updating state and emitting events.
fn process_tool_call_delta(
    tc_delta: &OaiToolCallDelta,
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
        // Existing tool call — append arguments
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

        // Finalize all pending tool calls
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

/// State machine tracking SSE streaming progress.
struct SseStreamState {
    text_started: bool,
    content_index: usize,
    tool_calls: HashMap<usize, ToolCallState>,
    usage: Option<Usage>,
    /// Saved stop reason from `finish_reason`; emitted with `Done` on `[DONE]`.
    stop_reason: Option<StopReason>,
}

// ─── Compile-time assertions ────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OpenAiStreamFn>();
};
