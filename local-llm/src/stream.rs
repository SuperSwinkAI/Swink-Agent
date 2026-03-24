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

use crate::model::{InternalModelState, LocalModel};

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
            return stream::iter(vec![AssistantMessageEvent::error(format!(
                "local model not ready: {e}"
            ))])
            .left_stream();
        }

        // Check for early cancellation before inference.
        if cancellation_token.is_cancelled() {
            return stream::iter(vec![
                AssistantMessageEvent::Start,
                AssistantMessageEvent::Done {
                    stop_reason: StopReason::Stop,
                    usage: Usage::default(),
                    cost: Cost::default(),
                },
            ])
            .left_stream();
        }

        // Step 2: Convert context to mistral.rs format.
        let messages = crate::convert::convert_context_messages(context);

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
                return stream::iter(vec![AssistantMessageEvent::error(format!(
                    "model runner unavailable: {e}"
                ))])
                .left_stream();
            }
        };

        let InternalModelState::Ready { runner } = &*state_guard else {
            return stream::iter(vec![AssistantMessageEvent::error(
                "model in unexpected state",
            )])
            .left_stream();
        };

        // Use non-streaming request and convert to events.
        // mistral.rs streaming API uses channels; we wrap the response
        // into the event protocol.
        let response = match runner.send_chat_request(messages).await {
            Ok(resp) => resp,
            Err(e) => {
                error!(error = %e, "local inference failed");
                return stream::iter(vec![AssistantMessageEvent::error(format!(
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
        events.push(AssistantMessageEvent::error("no choices in response"));
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
        total: u64::try_from(response.usage.total_tokens).unwrap_or(0),
        ..Default::default()
    };

    events.push(AssistantMessageEvent::Done {
        stop_reason,
        usage,
        // Local inference — no cost.
        cost: Cost::default(),
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

    use mistralrs::{
        CalledFunction, ChatCompletionResponse, Choice, ResponseMessage, ToolCallResponse,
        ToolCallType, Usage as MistralUsage,
    };

    // ── Helpers ──────────────────────────────────────────────────────────

    fn make_usage(prompt: usize, completion: usize) -> MistralUsage {
        MistralUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            avg_tok_per_sec: 0.0,
            avg_prompt_tok_per_sec: 0.0,
            avg_compl_tok_per_sec: 0.0,
            total_time_sec: 0.0,
            total_prompt_time_sec: 0.0,
            total_completion_time_sec: 0.0,
        }
    }

    fn make_response(choices: Vec<Choice>, usage: MistralUsage) -> ChatCompletionResponse {
        ChatCompletionResponse {
            id: "test-id".to_string(),
            choices,
            created: 0,
            model: "test-model".to_string(),
            system_fingerprint: "local".to_string(),
            object: "chat.completion".to_string(),
            usage,
        }
    }

    fn text_choice(text: &str, finish_reason: &str) -> Choice {
        Choice {
            finish_reason: finish_reason.to_string(),
            index: 0,
            message: ResponseMessage {
                content: Some(text.to_string()),
                role: "assistant".to_string(),
                tool_calls: None,
                reasoning_content: None,
            },
            logprobs: None,
        }
    }

    fn tool_call_choice(
        text: Option<&str>,
        tool_calls: Vec<ToolCallResponse>,
        finish_reason: &str,
    ) -> Choice {
        Choice {
            finish_reason: finish_reason.to_string(),
            index: 0,
            message: ResponseMessage {
                content: text.map(ToString::to_string),
                role: "assistant".to_string(),
                tool_calls: Some(tool_calls),
                reasoning_content: None,
            },
            logprobs: None,
        }
    }

    fn make_tool_call(id: &str, name: &str, args: &str) -> ToolCallResponse {
        ToolCallResponse {
            index: 0,
            id: id.to_string(),
            tp: ToolCallType::Function,
            function: CalledFunction {
                name: name.to_string(),
                arguments: args.to_string(),
            },
        }
    }

    // ── response_to_events tests ─────────────────────────────────────────

    #[test]
    fn response_text_only() {
        let response = make_response(
            vec![text_choice("Hello, world!", "stop")],
            make_usage(10, 5),
        );
        let token = CancellationToken::new();
        let events = response_to_events(&response, token);

        assert_eq!(events.len(), 5);
        assert!(matches!(events[0], AssistantMessageEvent::Start));
        assert!(matches!(
            events[1],
            AssistantMessageEvent::TextStart { content_index: 0 }
        ));
        match &events[2] {
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                assert_eq!(*content_index, 0);
                assert_eq!(delta, "Hello, world!");
            }
            other => panic!("expected TextDelta, got {other:?}"),
        }
        assert!(matches!(
            events[3],
            AssistantMessageEvent::TextEnd { content_index: 0 }
        ));
        match &events[4] {
            AssistantMessageEvent::Done {
                stop_reason, usage, ..
            } => {
                assert_eq!(*stop_reason, StopReason::Stop);
                assert_eq!(usage.input, 10);
                assert_eq!(usage.output, 5);
                assert_eq!(usage.total, 15);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn response_with_tool_calls() {
        let tc = make_tool_call("call-1", "read_file", r#"{"path": "/tmp/foo"}"#);
        let response = make_response(
            vec![tool_call_choice(None, vec![tc], "stop")],
            make_usage(20, 10),
        );
        let token = CancellationToken::new();
        let events = response_to_events(&response, token);

        // Start + ToolCallStart + ToolCallDelta + ToolCallEnd + Done = 5
        assert_eq!(events.len(), 5);
        assert!(matches!(events[0], AssistantMessageEvent::Start));
        match &events[1] {
            AssistantMessageEvent::ToolCallStart {
                content_index,
                id,
                name,
            } => {
                assert_eq!(*content_index, 0);
                assert_eq!(id, "call-1");
                assert_eq!(name, "read_file");
            }
            other => panic!("expected ToolCallStart, got {other:?}"),
        }
        match &events[2] {
            AssistantMessageEvent::ToolCallDelta {
                content_index,
                delta,
            } => {
                assert_eq!(*content_index, 0);
                assert_eq!(delta, r#"{"path": "/tmp/foo"}"#);
            }
            other => panic!("expected ToolCallDelta, got {other:?}"),
        }
        assert!(matches!(
            events[3],
            AssistantMessageEvent::ToolCallEnd { content_index: 0 }
        ));
        match &events[4] {
            AssistantMessageEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::ToolUse);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn response_with_thinking_tags() {
        let content = "<think>I should think carefully.</think>The answer is 42.";
        let response = make_response(vec![text_choice(content, "stop")], make_usage(5, 15));
        let token = CancellationToken::new();
        let events = response_to_events(&response, token);

        // Start + ThinkingStart + ThinkingDelta + ThinkingEnd
        //       + TextStart + TextDelta + TextEnd + Done = 8
        assert_eq!(events.len(), 8);
        assert!(matches!(events[0], AssistantMessageEvent::Start));

        // Thinking block at content_index 0.
        assert!(matches!(
            events[1],
            AssistantMessageEvent::ThinkingStart { content_index: 0 }
        ));
        match &events[2] {
            AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                assert_eq!(*content_index, 0);
                assert_eq!(delta, "I should think carefully.");
            }
            other => panic!("expected ThinkingDelta, got {other:?}"),
        }
        assert!(matches!(
            events[3],
            AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                ..
            }
        ));

        // Text block at content_index 1.
        assert!(matches!(
            events[4],
            AssistantMessageEvent::TextStart { content_index: 1 }
        ));
        match &events[5] {
            AssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                assert_eq!(*content_index, 1);
                assert_eq!(delta, "The answer is 42.");
            }
            other => panic!("expected TextDelta, got {other:?}"),
        }
        assert!(matches!(
            events[6],
            AssistantMessageEvent::TextEnd { content_index: 1 }
        ));
        assert!(matches!(events[7], AssistantMessageEvent::Done { .. }));
    }

    #[test]
    fn response_no_choices() {
        let response = make_response(vec![], make_usage(5, 0));
        let token = CancellationToken::new();
        let events = response_to_events(&response, token);

        // Start + Error = 2
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], AssistantMessageEvent::Start));
        match &events[1] {
            AssistantMessageEvent::Error { error_message, .. } => {
                assert!(error_message.contains("no choices"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn response_finish_reason_length() {
        let response = make_response(
            vec![text_choice("truncated output", "length")],
            make_usage(100, 50),
        );
        let token = CancellationToken::new();
        let events = response_to_events(&response, token);

        let done = events.last().expect("should have events");
        match done {
            AssistantMessageEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::Length);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn response_finish_reason_stop() {
        let response = make_response(
            vec![text_choice("complete output", "stop")],
            make_usage(50, 25),
        );
        let token = CancellationToken::new();
        let events = response_to_events(&response, token);

        let done = events.last().expect("should have events");
        match done {
            AssistantMessageEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::Stop);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn response_with_usage() {
        let response = make_response(vec![text_choice("hi", "stop")], make_usage(42, 13));
        let token = CancellationToken::new();
        let events = response_to_events(&response, token);

        let done = events.last().expect("should have events");
        match done {
            AssistantMessageEvent::Done { usage, cost, .. } => {
                assert_eq!(usage.input, 42);
                assert_eq!(usage.output, 13);
                assert_eq!(usage.total, 55);
                assert_eq!(usage.cache_read, 0);
                assert_eq!(usage.cache_write, 0);
                // Local inference cost is always zero.
                assert!(cost.total.abs() < f64::EPSILON);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn response_text_and_tool_calls() {
        let tc = make_tool_call("call-2", "bash", r#"{"cmd": "ls"}"#);
        let response = make_response(
            vec![tool_call_choice(Some("Let me check."), vec![tc], "stop")],
            make_usage(30, 20),
        );
        let token = CancellationToken::new();
        let events = response_to_events(&response, token);

        // Start + TextStart + TextDelta + TextEnd
        //       + ToolCallStart + ToolCallDelta + ToolCallEnd + Done = 8
        assert_eq!(events.len(), 8);

        // Text at content_index 0.
        assert!(matches!(
            events[1],
            AssistantMessageEvent::TextStart { content_index: 0 }
        ));

        // Tool call at content_index 1.
        match &events[4] {
            AssistantMessageEvent::ToolCallStart {
                content_index,
                name,
                ..
            } => {
                assert_eq!(*content_index, 1);
                assert_eq!(name, "bash");
            }
            other => panic!("expected ToolCallStart, got {other:?}"),
        }

        // Stop reason should be ToolUse when tool calls present.
        match &events[7] {
            AssistantMessageEvent::Done { stop_reason, .. } => {
                assert_eq!(*stop_reason, StopReason::ToolUse);
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn response_multiple_tool_calls() {
        let tc1 = make_tool_call("call-a", "read_file", r#"{"path": "a.rs"}"#);
        let tc2 = make_tool_call("call-b", "read_file", r#"{"path": "b.rs"}"#);
        let response = make_response(
            vec![tool_call_choice(None, vec![tc1, tc2], "stop")],
            make_usage(10, 10),
        );
        let token = CancellationToken::new();
        let events = response_to_events(&response, token);

        // Start + 2 * (ToolCallStart + ToolCallDelta + ToolCallEnd) + Done = 8
        assert_eq!(events.len(), 8);

        // First tool call at content_index 0.
        match &events[1] {
            AssistantMessageEvent::ToolCallStart {
                content_index, id, ..
            } => {
                assert_eq!(*content_index, 0);
                assert_eq!(id, "call-a");
            }
            other => panic!("expected ToolCallStart, got {other:?}"),
        }

        // Second tool call at content_index 1.
        match &events[4] {
            AssistantMessageEvent::ToolCallStart {
                content_index, id, ..
            } => {
                assert_eq!(*content_index, 1);
                assert_eq!(id, "call-b");
            }
            other => panic!("expected ToolCallStart, got {other:?}"),
        }
    }

    #[test]
    fn response_duplicate_tool_call_ids_deduplicated() {
        let tc1 = make_tool_call("same-id", "tool_a", "{}");
        let tc2 = make_tool_call("same-id", "tool_b", r#"{"x":1}"#);
        let response = make_response(
            vec![tool_call_choice(None, vec![tc1, tc2], "stop")],
            make_usage(5, 5),
        );
        let token = CancellationToken::new();
        let events = response_to_events(&response, token);

        // Duplicate id should be deduplicated: Start + 1 tool call (3 events) + Done = 5
        assert_eq!(events.len(), 5);
    }

    #[test]
    fn response_empty_text_no_text_block() {
        let choice = Choice {
            finish_reason: "stop".to_string(),
            index: 0,
            message: ResponseMessage {
                content: Some(String::new()),
                role: "assistant".to_string(),
                tool_calls: None,
                reasoning_content: None,
            },
            logprobs: None,
        };
        let response = make_response(vec![choice], make_usage(5, 0));
        let token = CancellationToken::new();
        let events = response_to_events(&response, token);

        // Start + Done = 2 (no text block emitted for empty content).
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], AssistantMessageEvent::Start));
        assert!(matches!(events[1], AssistantMessageEvent::Done { .. }));
    }

    #[test]
    fn response_none_content_no_text_block() {
        let choice = Choice {
            finish_reason: "stop".to_string(),
            index: 0,
            message: ResponseMessage {
                content: None,
                role: "assistant".to_string(),
                tool_calls: None,
                reasoning_content: None,
            },
            logprobs: None,
        };
        let response = make_response(vec![choice], make_usage(5, 0));
        let token = CancellationToken::new();
        let events = response_to_events(&response, token);

        // Start + Done = 2.
        assert_eq!(events.len(), 2);
    }

    // ── extract_thinking tests ───────────────────────────────────────────

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
        assert_eq!(thinking.as_deref(), Some("I need to reason about this."));
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
