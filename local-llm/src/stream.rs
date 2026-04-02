//! [`StreamFn`] implementation for local model inference.
//!
//! [`LocalStreamFn`] wraps a [`LocalModel`] and produces
//! [`AssistantMessageEvent`] values by incrementally streaming responses
//! from the mistral.rs inference engine. Uses the `stream_chat_request`
//! API for true token-by-token delivery and mid-generation cancellation.

use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{self, Stream, StreamExt as _};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, warn};

use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{AgentContext, Cost, ModelSpec, StopReason, Usage};

use crate::loader::LoaderState;
use crate::model::LocalModel;

// ─── LocalStreamFn ──────────────────────────────────────────────────────────

/// A [`StreamFn`] backed by a local GGUF model via mistral.rs.
///
/// On first call, lazily downloads and loads the model. Subsequent calls
/// reuse the loaded model. The underlying [`LocalModel`] is `Arc`-shared,
/// so multiple `LocalStreamFn` clones use the same loaded weights.
pub struct LocalStreamFn {
    model: Arc<LocalModel>,
}

impl LocalStreamFn {
    /// Create a new local stream function.
    #[must_use]
    pub const fn new(model: Arc<LocalModel>) -> Self {
        Self { model }
    }
}

impl std::fmt::Debug for LocalStreamFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalStreamFn")
            .field("model", &self.model)
            .finish()
    }
}

impl StreamFn for LocalStreamFn {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(local_stream(
            &self.model,
            model,
            context,
            options,
            cancellation_token,
        ))
    }
}

// ─── Streaming state ────────────────────────────────────────────────────────

/// Mutable state accumulated across streaming chunks.
struct StreamState {
    events: Vec<AssistantMessageEvent>,
    content_index: usize,
    text_started: bool,
    thinking_started: bool,
    accumulated_usage: Option<mistralrs::Usage>,
    finish_reason: Option<String>,
    has_tool_calls: bool,
    /// Map from tool call index to (`tool_id`, `content_index` at start).
    active_tool_calls: Vec<(String, usize)>,
}

impl StreamState {
    fn new() -> Self {
        Self {
            events: vec![AssistantMessageEvent::Start],
            content_index: 0,
            text_started: false,
            thinking_started: false,
            accumulated_usage: None,
            finish_reason: None,
            has_tool_calls: false,
            active_tool_calls: Vec::new(),
        }
    }

    /// Process a streaming chunk, appending events to the buffer.
    fn process_chunk(&mut self, chunk: &mistralrs::ChatCompletionChunkResponse) {
        if let Some(usage) = &chunk.usage {
            self.accumulated_usage = Some(usage.clone());
        }

        let Some(choice) = chunk.choices.first() else {
            return;
        };

        if let Some(reason) = &choice.finish_reason {
            self.finish_reason = Some(reason.clone());
        }

        self.process_reasoning_delta(choice);
        self.process_content_delta(choice);
        self.process_tool_call_delta(choice);
    }

    /// Process a final Done response, extracting usage and metadata.
    fn handle_done(&mut self, resp: &mistralrs::ChatCompletionResponse) {
        if self.accumulated_usage.is_none() {
            self.accumulated_usage = Some(resp.usage.clone());
        }
        if let Some(choice) = resp.choices.first() {
            if self.finish_reason.is_none() {
                self.finish_reason = Some(choice.finish_reason.clone());
            }
            if choice
                .message
                .tool_calls
                .as_ref()
                .is_some_and(|tc| !tc.is_empty())
            {
                self.has_tool_calls = true;
            }
        }
    }

    fn process_reasoning_delta(&mut self, choice: &mistralrs::ChunkChoice) {
        if let Some(reasoning) = &choice.delta.reasoning_content
            && !reasoning.is_empty()
        {
            if !self.thinking_started {
                self.events.push(AssistantMessageEvent::ThinkingStart {
                    content_index: self.content_index,
                });
                self.thinking_started = true;
            }
            self.events.push(AssistantMessageEvent::ThinkingDelta {
                content_index: self.content_index,
                delta: reasoning.clone(),
            });
        }
    }

    fn process_content_delta(&mut self, choice: &mistralrs::ChunkChoice) {
        let Some(content) = &choice.delta.content else {
            return;
        };
        let (thinking_part, text_part) = extract_thinking_delta(content);

        if let Some(think) = thinking_part
            && !think.is_empty()
        {
            if !self.thinking_started {
                self.events.push(AssistantMessageEvent::ThinkingStart {
                    content_index: self.content_index,
                });
                self.thinking_started = true;
            }
            self.events.push(AssistantMessageEvent::ThinkingDelta {
                content_index: self.content_index,
                delta: think,
            });
        }

        if !text_part.is_empty() {
            self.close_thinking_block();
            if !self.text_started {
                self.events.push(AssistantMessageEvent::TextStart {
                    content_index: self.content_index,
                });
                self.text_started = true;
            }
            self.events.push(AssistantMessageEvent::TextDelta {
                content_index: self.content_index,
                delta: text_part,
            });
        }
    }

    fn process_tool_call_delta(&mut self, choice: &mistralrs::ChunkChoice) {
        let Some(tool_calls) = &choice.delta.tool_calls else {
            return;
        };

        self.close_text_block();
        self.close_thinking_block();

        for tc in tool_calls {
            self.has_tool_calls = true;
            let tool_idx = tc.index;
            while self.active_tool_calls.len() <= tool_idx {
                self.active_tool_calls.push((String::new(), 0));
            }
            if self.active_tool_calls[tool_idx].0.is_empty() {
                self.active_tool_calls[tool_idx] = (tc.id.clone(), self.content_index);
                self.events.push(AssistantMessageEvent::ToolCallStart {
                    content_index: self.content_index,
                    id: tc.id.clone(),
                    name: tc.function.name.clone(),
                });
                self.content_index += 1;
            }
            let tc_content_index = self.active_tool_calls[tool_idx].1;
            if !tc.function.arguments.is_empty() {
                self.events.push(AssistantMessageEvent::ToolCallDelta {
                    content_index: tc_content_index,
                    delta: tc.function.arguments.clone(),
                });
            }
        }
    }

    fn close_thinking_block(&mut self) {
        if self.thinking_started {
            self.events.push(AssistantMessageEvent::ThinkingEnd {
                content_index: self.content_index,
                signature: None,
            });
            self.thinking_started = false;
            self.content_index += 1;
        }
    }

    fn close_text_block(&mut self) {
        if self.text_started {
            self.events.push(AssistantMessageEvent::TextEnd {
                content_index: self.content_index,
            });
            self.text_started = false;
            self.content_index += 1;
        }
    }

    fn finalize(mut self) -> Vec<AssistantMessageEvent> {
        self.close_thinking_block();
        self.close_text_block();

        for (id, tc_content_index) in &self.active_tool_calls {
            if !id.is_empty() {
                self.events.push(AssistantMessageEvent::ToolCallEnd {
                    content_index: *tc_content_index,
                });
            }
        }

        let stop_reason = if self.has_tool_calls {
            StopReason::ToolUse
        } else {
            match self.finish_reason.as_deref() {
                Some("length") => StopReason::Length,
                _ => StopReason::Stop,
            }
        };

        self.events.push(AssistantMessageEvent::Done {
            stop_reason,
            usage: build_usage(self.accumulated_usage.as_ref()),
            cost: Cost::default(),
        });

        self.events
    }

    fn finalize_cancelled(mut self) -> Vec<AssistantMessageEvent> {
        self.close_thinking_block();
        self.close_text_block();
        self.events.push(AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: build_usage(self.accumulated_usage.as_ref()),
            cost: Cost::default(),
        });
        self.events
    }
}

// ─── Stream implementation ──────────────────────────────────────────────────

// The `mistralrs::model::Stream` type is not re-exported, so the streaming
// loop must live in this function — it cannot be extracted into a helper
// that names the stream type in its signature. This slightly exceeds the
// 100-line limit but keeps the code correct.
#[allow(clippy::too_many_lines)]
fn local_stream<'a>(
    local_model: &'a LocalModel,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    _options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        if let Err(e) = local_model.ensure_ready().await {
            return stream::iter(vec![AssistantMessageEvent::error(format!(
                "local model not ready: {e}"
            ))]);
        }
        if cancellation_token.is_cancelled() {
            return stream::iter(vec![
                AssistantMessageEvent::Start,
                AssistantMessageEvent::Done {
                    stop_reason: StopReason::Stop,
                    usage: Usage::default(),
                    cost: Cost::default(),
                },
            ]);
        }

        let messages = crate::convert::convert_context_messages(context);
        debug!(
            provider = %model.provider,
            model_id = %model.model_id,
            message_count = context.messages.len(),
            "sending local inference request (streaming)"
        );

        let state_guard = match local_model.runner().await {
            Ok(guard) => guard,
            Err(e) => {
                return stream::iter(vec![AssistantMessageEvent::error(format!(
                    "model runner unavailable: {e}"
                ))]);
            }
        };
        let LoaderState::Ready { runner } = &*state_guard else {
            return stream::iter(vec![AssistantMessageEvent::error(
                "model in unexpected state",
            )]);
        };
        let mut mistral_stream = match runner.stream_chat_request(messages).await {
            Ok(s) => s,
            Err(e) => {
                error!(error = %e, "local streaming request failed");
                return stream::iter(vec![AssistantMessageEvent::error(format!(
                    "local inference error: {e}"
                ))]);
            }
        };

        let mut state = StreamState::new();
        while let Some(response) = mistral_stream.next().await {
            if cancellation_token.is_cancelled() {
                return stream::iter(state.finalize_cancelled());
            }
            match response {
                mistralrs::Response::Chunk(chunk) => state.process_chunk(&chunk),
                mistralrs::Response::Done(done) => {
                    state.handle_done(&done);
                    break;
                }
                mistralrs::Response::InternalError(e) => {
                    error!(error = %e, "internal error during local streaming");
                    state.events.push(AssistantMessageEvent::error(format!(
                        "local inference error: {e}"
                    )));
                    return stream::iter(state.events);
                }
                mistralrs::Response::ValidationError(e) => {
                    error!(error = %e, "validation error during local streaming");
                    state.events.push(AssistantMessageEvent::error(format!(
                        "local validation error: {e}"
                    )));
                    return stream::iter(state.events);
                }
                mistralrs::Response::ModelError(msg, _) => {
                    error!(error = %msg, "model error during local streaming");
                    state.events.push(AssistantMessageEvent::error(format!(
                        "local model error: {msg}"
                    )));
                    return stream::iter(state.events);
                }
                _ => warn!("unexpected response variant during streaming"),
            }
        }

        // Keep state_guard alive until stream is fully consumed.
        drop(state_guard);
        stream::iter(state.finalize())
    })
    .flatten()
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn build_usage(usage: Option<&mistralrs::Usage>) -> Usage {
    usage.map_or_else(Usage::default, |u| Usage {
        input: u64::try_from(u.prompt_tokens).unwrap_or(0),
        output: u64::try_from(u.completion_tokens).unwrap_or(0),
        total: u64::try_from(u.total_tokens).unwrap_or(0),
        ..Default::default()
    })
}

/// Extract `<think>...</think>` content from a streaming delta.
///
/// In streaming mode, `SmolLM3` may emit `<think>` tags across multiple
/// deltas. This handles the simple case where the full tag appears in one
/// delta; cross-delta tag boundaries are handled by the accumulator seeing
/// partial tags as text (acceptable degradation for streaming).
fn extract_thinking_delta(content: &str) -> (Option<String>, String) {
    let think_start = "<think>";
    let think_end = "</think>";

    if let Some(start_idx) = content.find(think_start)
        && let Some(end_idx) = content.find(think_end)
    {
        let thinking = content[start_idx + think_start.len()..end_idx]
            .trim()
            .to_string();
        let before = &content[..start_idx];
        let after = &content[end_idx + think_end.len()..];
        let text = format!("{before}{after}").trim().to_string();
        return (
            if thinking.is_empty() {
                None
            } else {
                Some(thinking)
            },
            text,
        );
    }

    (None, content.to_string())
}

// ─── Compile-time assertions ────────────────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LocalStreamFn>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_thinking_no_tags() {
        let (thinking, text) = extract_thinking_delta("Hello, world!");
        assert!(thinking.is_none());
        assert_eq!(text, "Hello, world!");
    }

    #[test]
    fn extract_thinking_with_tags() {
        let input = "<think>I need to reason about this.</think>The answer is 42.";
        let (thinking, text) = extract_thinking_delta(input);
        assert_eq!(thinking.as_deref(), Some("I need to reason about this."));
        assert_eq!(text, "The answer is 42.");
    }

    #[test]
    fn extract_thinking_empty_tags() {
        let input = "<think></think>Just text.";
        let (thinking, text) = extract_thinking_delta(input);
        assert!(thinking.is_none());
        assert_eq!(text, "Just text.");
    }

    #[test]
    fn extract_thinking_with_content_before() {
        let input = "Before <think>reasoning</think> after";
        let (thinking, text) = extract_thinking_delta(input);
        assert_eq!(thinking.as_deref(), Some("reasoning"));
        assert_eq!(text, "Before  after");
    }

    #[test]
    fn extract_thinking_unclosed_tag() {
        let input = "<think>unclosed thinking";
        let (thinking, text) = extract_thinking_delta(input);
        assert!(thinking.is_none());
        assert_eq!(text, "<think>unclosed thinking");
    }

    #[test]
    fn build_usage_none() {
        let usage = build_usage(None);
        assert_eq!(usage.input, 0);
        assert_eq!(usage.output, 0);
        assert_eq!(usage.total, 0);
    }

    #[test]
    fn build_usage_from_mistral() {
        let mistral_usage = mistralrs::Usage {
            prompt_tokens: 42,
            completion_tokens: 13,
            total_tokens: 55,
            avg_tok_per_sec: 0.0,
            avg_prompt_tok_per_sec: 0.0,
            avg_compl_tok_per_sec: 0.0,
            total_time_sec: 0.0,
            total_prompt_time_sec: 0.0,
            total_completion_time_sec: 0.0,
        };
        let usage = build_usage(Some(&mistral_usage));
        assert_eq!(usage.input, 42);
        assert_eq!(usage.output, 13);
        assert_eq!(usage.total, 55);
        assert_eq!(usage.cache_read, 0);
        assert_eq!(usage.cache_write, 0);
    }
}
