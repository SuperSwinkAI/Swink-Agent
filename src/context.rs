//! Context compaction utilities for managing conversation history size.

use serde::{Deserialize, Serialize};

use crate::types::{AgentMessage, ContentBlock, LlmMessage};

// ─── Token Counter Trait ────────────────────────────────────────────────────

/// Pluggable token counting strategy.
///
/// Implement this trait to replace the built-in `chars / 4` heuristic with
/// tiktoken, a provider-native tokenizer, or any other counting scheme.
pub trait TokenCounter: Send + Sync {
    /// Return the estimated token count for a single message.
    fn count_tokens(&self, message: &AgentMessage) -> usize;
}

/// Default token counter using the `chars / 4` heuristic.
///
/// `LlmMessage` variants: sums character lengths of all text-bearing content
/// blocks and divides by 4. `CustomMessage` variants count as 100 tokens flat.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultTokenCounter;

impl TokenCounter for DefaultTokenCounter {
    fn count_tokens(&self, message: &AgentMessage) -> usize {
        match message {
            AgentMessage::Llm(llm) => {
                let chars: usize = content_blocks(llm)
                    .iter()
                    .map(|b| match b {
                        ContentBlock::Text { text } => text.len(),
                        ContentBlock::Thinking { thinking, .. } => thinking.len(),
                        ContentBlock::ToolCall { arguments, .. } => arguments.to_string().len(),
                        ContentBlock::Image { .. } => 0,
                        ContentBlock::Extension { data, .. } => data.to_string().len(),
                    })
                    .sum();
                chars / 4
            }
            AgentMessage::Custom(_) => 100,
        }
    }
}

/// Estimate token count using `chars / 4` for LLM messages, 100 flat for custom.
///
/// For pluggable counting, use a [`TokenCounter`] implementation instead.
pub fn estimate_tokens(msg: &AgentMessage) -> usize {
    DefaultTokenCounter.count_tokens(msg)
}

fn content_blocks(msg: &LlmMessage) -> &[ContentBlock] {
    match msg {
        LlmMessage::User(m) => &m.content,
        LlmMessage::Assistant(m) => &m.content,
        LlmMessage::ToolResult(m) => &m.content,
    }
}

fn is_tool_result(messages: &[AgentMessage], idx: usize) -> bool {
    matches!(
        messages.get(idx),
        Some(AgentMessage::Llm(LlmMessage::ToolResult(_)))
    )
}

fn tool_call_ids(message: &AgentMessage) -> Option<Vec<&str>> {
    match message {
        AgentMessage::Llm(LlmMessage::Assistant(assistant)) => {
            let ids: Vec<&str> = assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolCall { id, .. } => Some(id.as_str()),
                    _ => None,
                })
                .collect();
            (!ids.is_empty()).then_some(ids)
        }
        _ => None,
    }
}

fn tool_result_id(message: &AgentMessage) -> Option<&str> {
    match message {
        AgentMessage::Llm(LlmMessage::ToolResult(result)) => Some(result.tool_call_id.as_str()),
        _ => None,
    }
}

fn extend_anchor_for_tool_results(messages: &[AgentMessage], anchor_end: usize) -> usize {
    if anchor_end == 0 || anchor_end >= messages.len() {
        return anchor_end;
    }

    let mut assistant_idx = anchor_end - 1;
    while is_tool_result(messages, assistant_idx) {
        if assistant_idx == 0 {
            return anchor_end;
        }
        assistant_idx -= 1;
    }

    let Some(call_ids) = tool_call_ids(&messages[assistant_idx]) else {
        return anchor_end;
    };

    let mut group_end = assistant_idx + 1;
    while group_end < messages.len() {
        let Some(result_id) = tool_result_id(&messages[group_end]) else {
            break;
        };
        if !call_ids.contains(&result_id) {
            break;
        }
        group_end += 1;
    }

    anchor_end.max(group_end)
}

/// Result of a context transformation pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionReport {
    /// Number of messages that were removed during compaction.
    pub dropped_count: usize,
    /// Estimated tokens before compaction.
    pub tokens_before: usize,
    /// Estimated tokens after compaction.
    pub tokens_after: usize,
    /// Whether compaction was triggered by overflow.
    pub overflow: bool,
    /// The LLM messages that were dropped during this compaction pass.
    ///
    /// Only `LlmMessage` variants are included; `CustomMessage` values are
    /// filtered out. Populated by the sliding-window compaction routine; empty for
    /// bare-closure transformers that don't have access to the dropped slice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_messages: Vec<LlmMessage>,
}

/// Core sliding window compaction algorithm.
///
/// Keeps messages within `budget` by removing older messages from the middle
/// while preserving the first `anchor` messages and as many recent messages as
/// fit. Tool-call / tool-result pairs are kept together even if this exceeds
/// the budget.
///
/// When `counter` is `None` the [`DefaultTokenCounter`] heuristic is used.
///
/// Returns `Some(CompactionReport)` when messages were dropped, `None` otherwise.
pub fn compact_sliding_window(
    messages: &mut Vec<AgentMessage>,
    budget: usize,
    anchor: usize,
) -> Option<CompactionReport> {
    compact_sliding_window_with(messages, budget, anchor, None)
}

/// Like [`compact_sliding_window`] but accepts a pluggable [`TokenCounter`].
pub fn compact_sliding_window_with(
    messages: &mut Vec<AgentMessage>,
    budget: usize,
    anchor: usize,
    counter: Option<&dyn TokenCounter>,
) -> Option<CompactionReport> {
    let default = DefaultTokenCounter;
    let counter: &dyn TokenCounter = counter.unwrap_or(&default);

    let count = |m: &AgentMessage| counter.count_tokens(m);

    let tokens_before: usize = messages.iter().map(count).sum();
    if tokens_before <= budget {
        return None;
    }

    let len = messages.len();
    let effective_anchor = extend_anchor_for_tool_results(messages, anchor.min(len));

    // Calculate tokens used by anchor messages.
    let anchor_tokens: usize = messages[..effective_anchor].iter().map(count).sum();

    let remaining_budget = budget.saturating_sub(anchor_tokens);

    // Walk backwards from the end, accumulating messages that fit.
    let mut tail_tokens = 0;
    let mut tail_start = len;

    for i in (effective_anchor..len).rev() {
        let msg_tokens = count(&messages[i]);
        if tail_tokens + msg_tokens > remaining_budget {
            break;
        }
        tail_tokens += msg_tokens;
        tail_start = i;
    }

    // Adjust tail_start backward to avoid splitting tool-call / tool-result
    // pairs. If tail_start lands on a tool-result, include the preceding
    // assistant message too (correctness > token count).
    while tail_start > effective_anchor && tail_start < len && is_tool_result(messages, tail_start)
    {
        tail_start -= 1;
    }

    // If nothing would be removed, bail out.
    if tail_start <= effective_anchor {
        return None;
    }

    let dropped_count = tail_start - effective_anchor;

    // Collect the dropped LLM messages before modifying the slice.
    let dropped_messages: Vec<LlmMessage> = messages[effective_anchor..tail_start]
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(llm) => Some(llm.clone()),
            AgentMessage::Custom(_) => None,
        })
        .collect();

    // Build the compacted list: anchor messages + tail messages.
    let tail: Vec<AgentMessage> = messages.drain(tail_start..).collect();
    messages.truncate(effective_anchor);
    messages.extend(tail);

    let tokens_after: usize = messages.iter().map(count).sum();

    Some(CompactionReport {
        dropped_count,
        tokens_before,
        tokens_after,
        overflow: false,
        dropped_messages,
    })
}

/// Create a sliding-window context compaction function.
///
/// Keeps messages within an estimated token budget by removing older messages
/// from the middle while preserving the first `anchor` messages and as many
/// recent messages as fit.
///
/// When `overflow` is true (context window exceeded), uses `overflow_budget`
/// instead of `normal_budget`.
#[deprecated(since = "0.5.0", note = "Use SlidingWindowTransformer instead")]
pub fn sliding_window(
    normal_budget: usize,
    overflow_budget: usize,
    anchor: usize,
) -> impl Fn(&mut Vec<AgentMessage>, bool) + Send + Sync {
    move |messages: &mut Vec<AgentMessage>, overflow: bool| {
        let budget = if overflow {
            overflow_budget
        } else {
            normal_budget
        };
        compact_sliding_window(messages, budget, anchor);
    }
}

/// Estimate whether the context exceeds the model's maximum context window.
///
/// Returns `true` if the estimated token count exceeds
/// `model.capabilities.max_context_window`. Returns `false` if the model has
/// no known context window limit.
pub fn is_context_overflow(
    messages: &[AgentMessage],
    model: &crate::types::ModelSpec,
    counter: Option<&dyn TokenCounter>,
) -> bool {
    let max_window = model
        .capabilities
        .as_ref()
        .and_then(|c| c.max_context_window);

    let Some(max_window) = max_window else {
        return false;
    };

    let default = DefaultTokenCounter;
    let counter: &dyn TokenCounter = counter.unwrap_or(&default);

    let total_tokens: usize = messages.iter().map(|m| counter.count_tokens(m)).sum();
    total_tokens as u64 > max_window
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason, ToolResultMessage, Usage,
        UserMessage,
    };

    fn text_message(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))
    }

    /// Helper: create an assistant message with a tool call.
    fn tool_call_message(id: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
            content: vec![ContentBlock::ToolCall {
                id: id.into(),
                name: "test".into(),
                arguments: serde_json::json!({}),
                partial_json: None,
            }],
            provider: String::new(),
            model_id: String::new(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        }))
    }

    fn multi_tool_call_message(ids: &[&str]) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
            content: ids
                .iter()
                .map(|id| ContentBlock::ToolCall {
                    id: (*id).into(),
                    name: "test".into(),
                    arguments: serde_json::json!({}),
                    partial_json: None,
                })
                .collect(),
            provider: String::new(),
            model_id: String::new(),
            usage: Usage::default(),
            cost: Cost::default(),
            stop_reason: StopReason::ToolUse,
            error_message: None,
            error_kind: None,
            timestamp: 0,
            cache_hint: None,
        }))
    }

    /// Helper: create a tool result message.
    fn tool_result_message(id: &str, text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            content: vec![ContentBlock::Text { text: text.into() }],
            is_error: false,
            timestamp: 0,
            details: serde_json::Value::Null,
            cache_hint: None,
        }))
    }

    #[test]
    #[allow(deprecated)]
    fn under_budget_no_change() {
        let compact = sliding_window(10_000, 5_000, 1);
        let mut messages = vec![text_message("hello"), text_message("world")];
        compact(&mut messages, false);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    #[allow(deprecated)]
    fn over_budget_trims_middle() {
        // Each message: 400 chars / 4 = 100 tokens.
        let body = "x".repeat(400);
        let compact = sliding_window(250, 100, 1);
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];
        compact(&mut messages, false);
        // Anchor (1 msg = 100 tokens) + as many recent as fit in 150 tokens = 1
        // So we keep anchor + 1 recent = 2 messages.
        assert_eq!(messages.len(), 2);
    }

    #[test]
    #[allow(deprecated)]
    fn overflow_uses_smaller_budget() {
        let body = "x".repeat(400);
        let compact = sliding_window(1000, 150, 1);
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];
        // Under normal budget (1000), total is 400 tokens — no trim.
        compact(&mut messages, false);
        assert_eq!(messages.len(), 4);

        // Under overflow budget (150), should trim.
        compact(&mut messages, true);
        assert!(messages.len() < 4);
    }

    #[test]
    #[allow(deprecated)]
    fn preserves_tool_result_pair() {
        let compact = sliding_window(300, 100, 1);

        let body = "x".repeat(400);
        let mut messages = vec![
            text_message(&body), // anchor
            text_message(&body), // will be removed
            tool_call_message("tc1"),
            tool_result_message("tc1", "result"),
        ];

        compact(&mut messages, false);

        // The tool result and its preceding assistant should stay together.
        let has_result = messages
            .iter()
            .any(|m| matches!(m, AgentMessage::Llm(LlmMessage::ToolResult(_))));
        let has_call = messages.iter().any(|m| {
            matches!(m, AgentMessage::Llm(LlmMessage::Assistant(a))
                if a.content.iter().any(|b| matches!(b, ContentBlock::ToolCall { .. })))
        });
        // If we kept the tool result, the tool call must be there too.
        if has_result {
            assert!(has_call);
        }
    }

    // ── New edge case tests ─────────────────────────────────────────────────

    #[test]
    #[allow(deprecated)]
    fn empty_messages_no_change() {
        let compact = sliding_window(100, 50, 1);
        let mut messages: Vec<AgentMessage> = vec![];
        compact(&mut messages, false);
        assert!(messages.is_empty());
    }

    #[test]
    #[allow(deprecated)]
    fn single_message_preserved() {
        // A single message should never be trimmed, even if it exceeds the budget.
        // With anchor=1, this message is the anchor and tail_start <= effective_anchor
        // causes an early return.
        let body = "x".repeat(4000); // 1000 tokens, budget is only 10
        let compact = sliding_window(10, 5, 1);
        let mut messages = vec![text_message(&body)];
        compact(&mut messages, false);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    #[allow(deprecated)]
    fn anchor_messages_always_kept() {
        // Even when anchors alone exceed the budget, they must be preserved.
        let body = "x".repeat(400); // 100 tokens each
        let compact = sliding_window(50, 25, 2); // budget < anchor cost

        let mut messages = vec![
            text_message(&body), // anchor 1
            text_message(&body), // anchor 2
            text_message(&body), // non-anchor
            text_message(&body), // non-anchor
        ];
        compact(&mut messages, false);

        // First two anchor messages must survive.
        assert!(messages.len() >= 2);
        // Verify the anchor messages are the originals by checking content length.
        for msg in &messages[..2] {
            if let AgentMessage::Llm(LlmMessage::User(u)) = msg {
                assert_eq!(u.content[0], ContentBlock::Text { text: body.clone() });
            } else {
                panic!("expected user message in anchor position");
            }
        }
    }

    #[test]
    #[allow(deprecated)]
    fn all_messages_under_budget_with_large_system_prompt() {
        // The sliding_window function operates on messages only; the system prompt
        // is not passed to it. Verify that when total message tokens are under
        // budget, nothing is trimmed regardless of external system prompt size.
        let compact = sliding_window(500, 250, 1);
        let mut messages = vec![
            text_message(&"a".repeat(400)), // 100 tokens
            text_message(&"b".repeat(400)), // 100 tokens
        ];
        // Total = 200 tokens, well under 500 budget.
        compact(&mut messages, false);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    #[allow(deprecated)]
    fn tool_result_at_boundary_preserved() {
        // When the natural trim point falls exactly on a tool-result message,
        // the preceding tool-call must also be kept.
        let body = "x".repeat(400); // 100 tokens each
        // Budget: anchor (100) + remaining 150 => can fit 1 message from tail.
        // Without tool-pair preservation, only the tool result would be kept.
        // With preservation, both the assistant (tool call) and result are kept.
        let compact = sliding_window(250, 100, 1);
        let mut messages = vec![
            text_message(&body),               // anchor (100 tokens)
            text_message(&body),               // middle, will be removed
            tool_call_message("tc1"),          // assistant with tool call
            tool_result_message("tc1", &body), // tool result (100 tokens)
        ];
        compact(&mut messages, false);

        let has_result = messages
            .iter()
            .any(|m| matches!(m, AgentMessage::Llm(LlmMessage::ToolResult(_))));
        let has_call = messages.iter().any(|m| {
            matches!(m, AgentMessage::Llm(LlmMessage::Assistant(a))
                if a.content.iter().any(|b| matches!(b, ContentBlock::ToolCall { .. })))
        });
        if has_result {
            assert!(has_call, "tool result kept without its preceding tool call");
        }
    }

    #[test]
    #[allow(deprecated)]
    fn anchor_boundary_keeps_result_with_anchor_tool_call() {
        let body = "x".repeat(400); // 100 tokens each
        let compact = sliding_window(250, 100, 2);
        let mut messages = vec![
            text_message(&body),               // anchor
            tool_call_message("tc1"),          // anchor ends on tool call
            tool_result_message("tc1", &body), // must stay with anchor tool call
            text_message(&body),               // removable middle
            text_message(&body),               // removable tail candidate
        ];

        compact(&mut messages, false);

        let has_call = messages.iter().any(|message| {
            matches!(
                message,
                AgentMessage::Llm(LlmMessage::Assistant(assistant))
                    if assistant.content.iter().any(|block| matches!(
                        block,
                        ContentBlock::ToolCall { id, .. } if id == "tc1"
                    ))
            )
        });
        let has_result = messages.iter().any(|message| {
            matches!(
                message,
                AgentMessage::Llm(LlmMessage::ToolResult(result))
                    if result.tool_call_id == "tc1"
            )
        });

        assert!(has_call, "anchor tool call should still be present");
        assert!(
            has_result,
            "anchor-side compaction must keep the matching tool result"
        );
    }

    #[test]
    #[allow(deprecated)]
    fn anchor_boundary_keeps_all_results_for_multi_tool_call_message() {
        let body = "x".repeat(400); // 100 tokens each
        let compact = sliding_window(250, 100, 2);
        let mut messages = vec![
            text_message(&body),
            multi_tool_call_message(&["tc1", "tc2"]),
            tool_result_message("tc1", &body),
            tool_result_message("tc2", &body),
            text_message(&body),
        ];

        compact(&mut messages, false);

        let kept_results: Vec<&str> = messages
            .iter()
            .filter_map(|message| match message {
                AgentMessage::Llm(LlmMessage::ToolResult(result)) => {
                    Some(result.tool_call_id.as_str())
                }
                _ => None,
            })
            .collect();

        assert_eq!(kept_results, vec!["tc1", "tc2"]);
    }

    #[test]
    #[allow(deprecated)]
    fn anchor_boundary_inside_multi_tool_results_keeps_whole_group() {
        let body = "x".repeat(400); // 100 tokens each
        let compact = sliding_window(250, 100, 3);
        let mut messages = vec![
            text_message(&body),
            multi_tool_call_message(&["tc1", "tc2"]),
            tool_result_message("tc1", &body),
            tool_result_message("tc2", &body),
            text_message(&body),
        ];

        compact(&mut messages, false);

        let kept_results: Vec<&str> = messages
            .iter()
            .filter_map(|message| match message {
                AgentMessage::Llm(LlmMessage::ToolResult(result)) => {
                    Some(result.tool_call_id.as_str())
                }
                _ => None,
            })
            .collect();

        assert_eq!(kept_results, vec!["tc1", "tc2"]);
    }

    #[test]
    #[allow(deprecated)]
    fn consecutive_tool_pairs_preserved() {
        // Multiple consecutive tool call/result pairs at the tail should all
        // be kept together: if any tool result is included, its call is too.
        let compact = sliding_window(500, 100, 1);
        let body = "x".repeat(400); // 100 tokens each

        let mut messages = vec![
            text_message(&body),              // anchor
            text_message(&body),              // middle filler
            tool_call_message("tc1"),         // pair 1
            tool_result_message("tc1", "r1"), // pair 1
            tool_call_message("tc2"),         // pair 2
            tool_result_message("tc2", "r2"), // pair 2
        ];
        compact(&mut messages, false);

        // For every tool result in the output, verify its call is also present.
        for msg in &messages {
            if let AgentMessage::Llm(LlmMessage::ToolResult(tr)) = msg {
                let call_present = messages.iter().any(|m| {
                    matches!(m, AgentMessage::Llm(LlmMessage::Assistant(a))
                        if a.content.iter().any(|b| matches!(b, ContentBlock::ToolCall { id, .. } if id == &tr.tool_call_id)))
                });
                assert!(
                    call_present,
                    "tool result {} kept without its call",
                    tr.tool_call_id
                );
            }
        }
    }

    #[test]
    #[allow(deprecated)]
    fn custom_messages_token_estimation() {
        // CustomMessage uses 100 tokens flat, regardless of content.
        // Create a custom message and verify it contributes to budget.

        #[derive(Debug)]
        struct TestCustom;
        impl crate::types::CustomMessage for TestCustom {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }

        // Budget: 150 tokens. Two custom messages = 200 tokens => should trim.
        let compact = sliding_window(150, 50, 1);
        let mut messages: Vec<AgentMessage> = vec![
            AgentMessage::Custom(Box::new(TestCustom)), // anchor, 100 tokens
            AgentMessage::Custom(Box::new(TestCustom)), // 100 tokens
        ];
        // Total = 200 > 150, but with only 2 messages and anchor=1,
        // remaining budget = 150 - 100 = 50 < 100, so the second message is trimmed.
        compact(&mut messages, false);
        assert_eq!(messages.len(), 1);
    }

    #[test]
    #[allow(deprecated)]
    fn overflow_budget_smaller_than_normal() {
        // Verify that overflow mode trims more aggressively.
        let body = "x".repeat(400); // 100 tokens each
        let compact = sliding_window(350, 150, 1);

        // 4 messages = 400 tokens.
        // Normal budget (350): keeps anchor (100) + remaining 250 => 2 tail = 3 total.
        let mut normal_msgs = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];
        compact(&mut normal_msgs, false);
        let normal_count = normal_msgs.len();

        // Overflow budget (150): keeps anchor (100) + remaining 50 => 0 tail = 1 total.
        let mut overflow_msgs = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];
        compact(&mut overflow_msgs, true);
        let overflow_count = overflow_msgs.len();

        assert!(
            overflow_count < normal_count,
            "overflow budget ({overflow_count} msgs) should be more aggressive than normal ({normal_count} msgs)"
        );
    }

    // ── TokenCounter trait tests ────────────────────────────────────────────

    #[test]
    fn default_token_counter_matches_estimate_tokens() {
        let msg = text_message(&"x".repeat(400));
        assert_eq!(
            DefaultTokenCounter.count_tokens(&msg),
            estimate_tokens(&msg)
        );
        assert_eq!(DefaultTokenCounter.count_tokens(&msg), 100);
    }

    #[test]
    fn default_token_counter_custom_message_flat_100() {
        #[derive(Debug)]
        struct TestCustom;
        impl crate::types::CustomMessage for TestCustom {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }

        let msg = AgentMessage::Custom(Box::new(TestCustom));
        assert_eq!(DefaultTokenCounter.count_tokens(&msg), 100);
    }

    /// A custom counter that counts every character as one token.
    struct CharCounter;

    impl TokenCounter for CharCounter {
        fn count_tokens(&self, message: &AgentMessage) -> usize {
            match message {
                AgentMessage::Llm(llm) => content_blocks(llm)
                    .iter()
                    .map(|b| match b {
                        ContentBlock::Text { text } => text.len(),
                        _ => 0,
                    })
                    .sum(),
                AgentMessage::Custom(_) => 50,
            }
        }
    }

    #[test]
    fn custom_counter_used_by_compact_sliding_window_with() {
        // Each message: 400 chars. With CharCounter, that is 400 tokens each.
        // Budget 500 with anchor=1: anchor=400, remaining=100 < 400 => trim all non-anchor.
        let body = "x".repeat(400);
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];

        let result = compact_sliding_window_with(&mut messages, 500, 1, Some(&CharCounter));
        assert!(result.is_some());
        // Only anchor kept; remaining budget (100) cannot fit any 400-token message.
        assert_eq!(messages.len(), 1);
        let r = result.unwrap();
        assert_eq!(r.tokens_before, 1200);
        assert_eq!(r.tokens_after, 400);
    }

    #[test]
    fn custom_counter_no_compaction_when_under_budget() {
        // With CharCounter, 2 messages of 100 chars = 200 tokens. Budget 500 => no trim.
        let body = "x".repeat(100);
        let mut messages = vec![text_message(&body), text_message(&body)];

        let result = compact_sliding_window_with(&mut messages, 500, 1, Some(&CharCounter));
        assert!(result.is_none());
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn compact_sliding_window_backward_compat() {
        // The original compact_sliding_window still works with DefaultTokenCounter.
        let body = "x".repeat(400); // 100 tokens each
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];
        let result = compact_sliding_window(&mut messages, 250, 1);
        assert!(result.is_some());
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn compaction_report_includes_dropped_messages() {
        // Regression test for #164: CompactionReport.dropped_messages must be
        // populated by compact_sliding_window_with, not reconstructed via Debug diff.
        let body = "x".repeat(400); // 100 tokens each
        // anchor | dropped | dropped | tail
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];
        // Budget 250, anchor=1: anchor(100t) + tail(100t) = 200t fits; 2 middle dropped.
        let report = compact_sliding_window_with(&mut messages, 250, 1, None).unwrap();

        assert_eq!(report.dropped_count, 2);
        assert_eq!(report.dropped_messages.len(), 2);
        // Surviving: 2 messages.
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn compaction_report_dropped_messages_empty_when_no_compaction() {
        let mut messages = vec![text_message("hello"), text_message("world")];
        let result = compact_sliding_window_with(&mut messages, 10_000, 1, None);
        // No compaction — result is None, so dropped_messages never exists.
        assert!(result.is_none());
    }

    // ── is_context_overflow tests ──────────────────────────────────────

    fn model_with_window(window: u64) -> crate::types::ModelSpec {
        crate::types::ModelSpec {
            provider: "test".into(),
            model_id: "test-model".into(),
            thinking_level: crate::types::ThinkingLevel::default(),
            thinking_budgets: None,
            provider_config: None,
            capabilities: Some(
                crate::types::ModelCapabilities::none().with_max_context_window(window),
            ),
        }
    }

    fn model_no_window() -> crate::types::ModelSpec {
        crate::types::ModelSpec {
            provider: "test".into(),
            model_id: "test-model".into(),
            thinking_level: crate::types::ThinkingLevel::default(),
            thinking_budgets: None,
            provider_config: None,
            capabilities: None,
        }
    }

    #[test]
    fn overflow_within_budget_returns_false() {
        let messages = vec![text_message(&"x".repeat(400))]; // 100 tokens
        assert!(!is_context_overflow(
            &messages,
            &model_with_window(1000),
            None
        ));
    }

    #[test]
    fn overflow_exceeding_budget_returns_true() {
        let messages = vec![
            text_message(&"x".repeat(400)), // 100 tokens
            text_message(&"x".repeat(400)), // 100 tokens
        ];
        assert!(is_context_overflow(
            &messages,
            &model_with_window(150),
            None
        ));
    }

    #[test]
    fn overflow_no_window_returns_false() {
        let messages = vec![text_message(&"x".repeat(40_000))]; // 10_000 tokens
        assert!(!is_context_overflow(&messages, &model_no_window(), None));
    }

    #[test]
    fn overflow_custom_counter() {
        let messages = vec![text_message(&"x".repeat(400))]; // CharCounter: 400 tokens
        // With CharCounter and window=300, overflow should be detected
        assert!(is_context_overflow(
            &messages,
            &model_with_window(300),
            Some(&CharCounter)
        ));
        // With default counter: 400/4 = 100 tokens < 300
        assert!(!is_context_overflow(
            &messages,
            &model_with_window(300),
            None
        ));
    }

    #[test]
    fn overflow_empty_messages_returns_false() {
        let messages: Vec<AgentMessage> = vec![];
        assert!(!is_context_overflow(
            &messages,
            &model_with_window(100),
            None
        ));
    }
}
