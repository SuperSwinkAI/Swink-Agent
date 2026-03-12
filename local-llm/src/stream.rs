//! [`StreamFn`] implementation for local model inference.
//!
//! [`LocalStreamFn`] wraps a [`LocalModel`] and produces
//! [`AssistantMessageEvent`] values by streaming responses from the
//! mistral.rs inference engine. Follows the same stream state machine
//! pattern as the Ollama adapter.

use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{self, Stream, StreamExt as _};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

use swink_agent::stream::{AssistantMessageEvent, StreamFn, StreamOptions};
use swink_agent::types::{AgentContext, Cost, ModelSpec, StopReason, Usage};

use crate::model::{LocalModel, ModelState};

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

// ─── Stream implementation ──────────────────────────────────────────────────

fn local_stream<'a>(
    local_model: &'a LocalModel,
    model: &'a ModelSpec,
    context: &'a AgentContext,
    _options: &'a StreamOptions,
    cancellation_token: CancellationToken,
) -> impl Stream<Item = AssistantMessageEvent> + Send + 'a {
    stream::once(async move {
        // Step 1: Ensure model is downloaded and loaded.
        if let Err(e) = local_model.ensure_ready().await {
            return stream::iter(vec![error_event(&format!(
                "local model not ready: {e}"
            ))])
            .left_stream();
        }

        // Step 2: Convert context to mistral.rs format.
        let messages = crate::convert::convert_messages(context);

        debug!(
            provider = %model.provider,
            model_id = %model.model_id,
            message_count = context.messages.len(),
            tool_count = context.tools.len(),
            "sending local inference request"
        );

        // Step 3: Get the runner and send request.
        let state_guard = match local_model.runner().await {
            Ok(guard) => guard,
            Err(e) => {
                return stream::iter(vec![error_event(&format!(
                    "model runner unavailable: {e}"
                ))])
                .left_stream();
            }
        };

        let ModelState::Ready { runner } = &*state_guard else {
            return stream::iter(vec![error_event("model in unexpected state")])
                .left_stream();
        };

        // Use non-streaming request and convert to events.
        // mistral.rs streaming API uses channels; we wrap the response
        // into the event protocol.
        let response = match runner.send_chat_request(messages).await {
            Ok(resp) => resp,
            Err(e) => {
                error!(error = %e, "local inference failed");
                return stream::iter(vec![error_event(&format!(
                    "local inference error: {e}"
                ))])
                .left_stream();
            }
        };

        // Drop the read guard before building events.
        drop(state_guard);

        // Step 4: Convert response to AssistantMessageEvent sequence.
        let events = response_to_events(&response, cancellation_token);
        stream::iter(events).right_stream()
    })
    .flatten()
}

/// Convert a mistral.rs `ChatCompletionResponse` into a sequence of
/// `AssistantMessageEvent` values following the start/delta/end protocol.
fn response_to_events(
    response: &mistralrs::ChatCompletionResponse,
    _cancellation_token: CancellationToken,
) -> Vec<AssistantMessageEvent> {
    let mut events = Vec::new();
    events.push(AssistantMessageEvent::Start);

    let mut content_index: usize = 0;
    let mut thinking_text = String::new();
    let mut response_text = String::new();
    let mut tool_calls_emitted = HashSet::new();

    // Extract the first choice.
    let Some(choice) = response.choices.first() else {
        events.push(error_event("no choices in response"));
        return events;
    };

    // Get the response content.
    if let Some(content) = &choice.message.content {
        // Check for <think> tags (SmolLM3 thinking pattern).
        let (thinking, text) = extract_thinking(content);

        if let Some(think) = thinking {
            thinking_text = think;
        }
        response_text = text;
    }

    // Emit thinking block if present.
    if !thinking_text.is_empty() {
        events.push(AssistantMessageEvent::ThinkingStart { content_index });
        events.push(AssistantMessageEvent::ThinkingDelta {
            content_index,
            delta: thinking_text,
        });
        events.push(AssistantMessageEvent::ThinkingEnd {
            content_index,
            signature: None,
        });
        content_index += 1;
    }

    // Emit text block if present.
    if !response_text.is_empty() {
        events.push(AssistantMessageEvent::TextStart { content_index });
        events.push(AssistantMessageEvent::TextDelta {
            content_index,
            delta: response_text,
        });
        events.push(AssistantMessageEvent::TextEnd { content_index });
        content_index += 1;
    }

    // Emit tool call blocks.
    if let Some(tool_calls) = &choice.message.tool_calls {
        for tc in tool_calls {
            if tool_calls_emitted.insert(tc.id.clone()) {
                let tool_id = tc.id.clone();
                events.push(AssistantMessageEvent::ToolCallStart {
                    content_index,
                    id: tool_id,
                    name: tc.function.name.clone(),
                });
                events.push(AssistantMessageEvent::ToolCallDelta {
                    content_index,
                    delta: tc.function.arguments.clone(),
                });
                events.push(AssistantMessageEvent::ToolCallEnd { content_index });
                content_index += 1;
            }
        }
    }

    // Determine stop reason.
    let has_tool_calls = choice
        .message
        .tool_calls
        .as_ref()
        .is_some_and(|tc| !tc.is_empty());

    let stop_reason = if has_tool_calls {
        StopReason::ToolUse
    } else {
        match choice.finish_reason.as_str() {
            "length" => StopReason::Length,
            _ => StopReason::Stop,
        }
    };

    // Build usage from response.
    let usage = Usage {
        input: u64::try_from(response.usage.prompt_tokens).unwrap_or(0),
        output: u64::try_from(response.usage.completion_tokens).unwrap_or(0),
        cache_read: 0,
        cache_write: 0,
        total: u64::try_from(response.usage.total_tokens).unwrap_or(0),
    };

    events.push(AssistantMessageEvent::Done {
        stop_reason,
        usage,
        // Local inference — no cost.
        cost: Cost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            total: 0.0,
        },
    });

    events
}

/// Extract `<think>...</think>` content from `SmolLM3` responses.
///
/// `SmolLM3` uses `<think>` tags for chain-of-thought reasoning. This function
/// separates thinking content from the regular response text.
fn extract_thinking(content: &str) -> (Option<String>, String) {
    let think_start = "<think>";
    let think_end = "</think>";

    if let Some(start_idx) = content.find(think_start)
        && let Some(end_idx) = content.find(think_end)
    {
        let thinking =
            content[start_idx + think_start.len()..end_idx].trim().to_string();
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
    assert_send_sync::<LocalStreamFn>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_thinking_no_tags() {
        let (thinking, text) = extract_thinking("Hello, world!");
        assert!(thinking.is_none());
        assert_eq!(text, "Hello, world!");
    }

    #[test]
    fn extract_thinking_with_tags() {
        let input = "<think>I need to reason about this.</think>The answer is 42.";
        let (thinking, text) = extract_thinking(input);
        assert_eq!(
            thinking.as_deref(),
            Some("I need to reason about this.")
        );
        assert_eq!(text, "The answer is 42.");
    }

    #[test]
    fn extract_thinking_empty_tags() {
        let input = "<think></think>Just text.";
        let (thinking, text) = extract_thinking(input);
        assert!(thinking.is_none());
        assert_eq!(text, "Just text.");
    }

    #[test]
    fn extract_thinking_with_content_before() {
        let input = "Before <think>reasoning</think> after";
        let (thinking, text) = extract_thinking(input);
        assert_eq!(thinking.as_deref(), Some("reasoning"));
        assert_eq!(text, "Before  after");
    }

    #[test]
    fn extract_thinking_unclosed_tag() {
        let input = "<think>unclosed thinking";
        let (thinking, text) = extract_thinking(input);
        assert!(thinking.is_none());
        assert_eq!(text, "<think>unclosed thinking");
    }
}
