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
    use crate::types::{ContentBlock, LlmMessage, UserMessage};

    fn text_message(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
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
        use crate::types::{AssistantMessage, Cost, StopReason, ToolResultMessage, Usage};

        let compact = sliding_window(300, 100, 1);

        let body = "x".repeat(400);
        let mut messages = vec![
            text_message(&body), // anchor
            text_message(&body), // will be removed
            // Assistant with tool call
            AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
                content: vec![ContentBlock::ToolCall {
                    id: "tc1".into(),
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
            })),
            // Tool result
            AgentMessage::Llm(LlmMessage::ToolResult(ToolResultMessage {
                tool_call_id: "tc1".into(),
                content: vec![ContentBlock::Text {
                    text: "result".into(),
                }],
                is_error: false,
                timestamp: 0,
            })),
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
}
