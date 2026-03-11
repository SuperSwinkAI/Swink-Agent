//! Context compaction utilities for managing conversation history size.

use crate::types::{AgentMessage, ContentBlock, LlmMessage};

/// Estimate the token count of a single message.
///
/// Uses a simple heuristic: count characters in all text content blocks of
/// `LlmMessage` variants and divide by 4. `CustomMessage` variants count as
/// 100 tokens each.
fn estimate_tokens(msg: &AgentMessage) -> usize {
    match msg {
        AgentMessage::Llm(llm) => {
            let chars: usize = content_blocks(llm)
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text } => text.len(),
                    ContentBlock::Thinking { thinking, .. } => thinking.len(),
                    ContentBlock::ToolCall { arguments, .. } => arguments.to_string().len(),
                    ContentBlock::Image { .. } => 0,
                })
                .sum();
            chars / 4
        }
        AgentMessage::Custom(_) => 100,
    }
}

/// Extract content blocks from an `LlmMessage`.
fn content_blocks(msg: &LlmMessage) -> &[ContentBlock] {
    match msg {
        LlmMessage::User(m) => &m.content,
        LlmMessage::Assistant(m) => &m.content,
        LlmMessage::ToolResult(m) => &m.content,
    }
}

/// Returns true if the message at `idx` is a tool result.
fn is_tool_result(messages: &[AgentMessage], idx: usize) -> bool {
    matches!(
        messages.get(idx),
        Some(AgentMessage::Llm(LlmMessage::ToolResult(_)))
    )
}

/// Create a sliding-window context compaction function.
///
/// Keeps messages within an estimated token budget by removing older messages
/// from the middle while preserving the first `anchor` messages and as many
/// recent messages as fit.
///
/// When `overflow` is true (context window exceeded), uses `overflow_budget`
/// instead of `normal_budget`.
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

        let total_tokens: usize = messages.iter().map(estimate_tokens).sum();
        if total_tokens <= budget {
            return;
        }

        let len = messages.len();
        let effective_anchor = anchor.min(len);

        // Calculate tokens used by anchor messages.
        let anchor_tokens: usize = messages[..effective_anchor]
            .iter()
            .map(estimate_tokens)
            .sum();

        let remaining_budget = budget.saturating_sub(anchor_tokens);

        // Walk backwards from the end, accumulating messages that fit.
        let mut tail_tokens = 0;
        let mut tail_start = len;

        for i in (effective_anchor..len).rev() {
            let msg_tokens = estimate_tokens(&messages[i]);
            if tail_tokens + msg_tokens > remaining_budget {
                break;
            }
            tail_tokens += msg_tokens;
            tail_start = i;
        }

        // Adjust tail_start forward to avoid splitting tool-call / tool-result
        // pairs. If tail_start lands on a tool-result, we need the preceding
        // assistant message too (which may push us over budget, but
        // correctness matters more).
        while tail_start > effective_anchor
            && tail_start < len
            && is_tool_result(messages, tail_start)
        {
            tail_start -= 1;
        }

        // Also, if tail_start lands right after an assistant-with-tool-calls
        // whose results are being removed, include that assistant message's
        // tool results by checking forward.
        // Actually the simpler invariant: if the message at tail_start - 1 has
        // tool calls and tail_start is a tool result, we already handled it.
        // But we also must ensure we don't cut off an assistant message whose
        // tool results follow it: if messages[tail_start] is an assistant with
        // tool calls, make sure all subsequent tool results are included.
        // We already keep everything from tail_start to end, so this is
        // automatically satisfied.

        // If nothing would be removed, bail out.
        if tail_start <= effective_anchor {
            return;
        }

        // Build the compacted list: anchor messages + tail messages.
        let tail: Vec<AgentMessage> = messages.drain(tail_start..).collect();
        messages.truncate(effective_anchor);
        messages.extend(tail);
    }
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
            timestamp: 0,
        }))
    }

    /// Helper: create a tool result message.
    fn tool_result_message(id: &str, text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            content: vec![ContentBlock::Text {
                text: text.into(),
            }],
            is_error: false,
            timestamp: 0,
        }))
    }

    #[test]
    fn under_budget_no_change() {
        let compact = sliding_window(10_000, 5_000, 1);
        let mut messages = vec![text_message("hello"), text_message("world")];
        compact(&mut messages, false);
        assert_eq!(messages.len(), 2);
    }

    #[test]
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
    fn empty_messages_no_change() {
        let compact = sliding_window(100, 50, 1);
        let mut messages: Vec<AgentMessage> = vec![];
        compact(&mut messages, false);
        assert!(messages.is_empty());
    }

    #[test]
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
    fn tool_result_at_boundary_preserved() {
        // When the natural trim point falls exactly on a tool-result message,
        // the preceding tool-call must also be kept.
        let body = "x".repeat(400); // 100 tokens each
        // Budget: anchor (100) + remaining 150 => can fit 1 message from tail.
        // Without tool-pair preservation, only the tool result would be kept.
        // With preservation, both the assistant (tool call) and result are kept.
        let compact = sliding_window(250, 100, 1);
        let mut messages = vec![
            text_message(&body),             // anchor (100 tokens)
            text_message(&body),             // middle, will be removed
            tool_call_message("tc1"),        // assistant with tool call
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
    fn consecutive_tool_pairs_preserved() {
        // Multiple consecutive tool call/result pairs at the tail should all
        // be kept together: if any tool result is included, its call is too.
        let compact = sliding_window(500, 100, 1);
        let body = "x".repeat(400); // 100 tokens each

        let mut messages = vec![
            text_message(&body),                // anchor
            text_message(&body),                // middle filler
            tool_call_message("tc1"),           // pair 1
            tool_result_message("tc1", "r1"),   // pair 1
            tool_call_message("tc2"),           // pair 2
            tool_result_message("tc2", "r2"),   // pair 2
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
}
