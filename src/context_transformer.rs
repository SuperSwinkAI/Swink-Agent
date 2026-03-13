//! Pluggable context transformation with compaction awareness.
//!
//! Replaces the bare `TransformContextFn` closure with a trait that supports
//! both transformation and compaction reporting.

use crate::context::estimate_tokens;
use crate::types::{AgentMessage, LlmMessage};

/// Result of a context transformation pass.
#[derive(Debug, Clone)]
pub struct CompactionReport {
    /// Number of messages that were removed during compaction.
    pub dropped_count: usize,
    /// Estimated tokens before compaction.
    pub tokens_before: usize,
    /// Estimated tokens after compaction.
    pub tokens_after: usize,
    /// Whether compaction was triggered by overflow.
    pub overflow: bool,
}

/// Pluggable context transformation with compaction awareness.
///
/// Replaces the bare `TransformContextFn` closure with a trait that supports
/// both transformation and compaction reporting.
pub trait ContextTransformer: Send + Sync {
    /// Transform the context messages in-place.
    ///
    /// Called synchronously before each LLM call. The `overflow` flag is true
    /// when the previous turn exceeded the context window.
    ///
    /// Returns `Some(CompactionReport)` if messages were dropped, `None` otherwise.
    fn transform(
        &self,
        messages: &mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Option<CompactionReport>;
}

/// Blanket impl for existing closures (backward compat).
impl<F: Fn(&mut Vec<AgentMessage>, bool) + Send + Sync> ContextTransformer for F {
    fn transform(
        &self,
        messages: &mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Option<CompactionReport> {
        let before = messages.len();
        self(messages, overflow);
        let after = messages.len();
        if after < before {
            Some(CompactionReport {
                dropped_count: before - after,
                tokens_before: 0, // bare closures can't report token counts
                tokens_after: 0,
                overflow,
            })
        } else {
            None
        }
    }
}

/// Built-in sliding window context transformer with compaction reporting.
///
/// Wraps the same logic as [`sliding_window`](crate::sliding_window) but
/// captures compaction metrics for reporting.
pub struct SlidingWindowTransformer {
    normal_budget: usize,
    overflow_budget: usize,
    anchor: usize,
}

impl SlidingWindowTransformer {
    /// Create a new sliding window transformer.
    ///
    /// # Arguments
    ///
    /// * `normal_budget` - Token budget under normal operation.
    /// * `overflow_budget` - Smaller token budget used when overflow is signaled.
    /// * `anchor` - Number of messages at the start to always preserve.
    #[must_use]
    pub const fn new(normal_budget: usize, overflow_budget: usize, anchor: usize) -> Self {
        Self {
            normal_budget,
            overflow_budget,
            anchor,
        }
    }
}

/// Returns true if the message at `idx` is a tool result.
fn is_tool_result(messages: &[AgentMessage], idx: usize) -> bool {
    matches!(
        messages.get(idx),
        Some(AgentMessage::Llm(LlmMessage::ToolResult(_)))
    )
}

impl ContextTransformer for SlidingWindowTransformer {
    fn transform(
        &self,
        messages: &mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Option<CompactionReport> {
        let budget = if overflow {
            self.overflow_budget
        } else {
            self.normal_budget
        };

        let tokens_before: usize = messages.iter().map(estimate_tokens).sum();
        if tokens_before <= budget {
            return None;
        }

        let len = messages.len();
        let effective_anchor = self.anchor.min(len);

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
        // assistant message too.
        while tail_start > effective_anchor
            && tail_start < len
            && is_tool_result(messages, tail_start)
        {
            tail_start -= 1;
        }

        // If nothing would be removed, bail out.
        if tail_start <= effective_anchor {
            return None;
        }

        let dropped_count = tail_start - effective_anchor;

        // Build the compacted list: anchor messages + tail messages.
        let tail: Vec<AgentMessage> = messages.drain(tail_start..).collect();
        messages.truncate(effective_anchor);
        messages.extend(tail);

        let tokens_after: usize = messages.iter().map(estimate_tokens).sum();

        Some(CompactionReport {
            dropped_count,
            tokens_before,
            tokens_after,
            overflow,
        })
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
    fn sliding_window_transformer_reports_dropped_messages() {
        let transformer = SlidingWindowTransformer::new(250, 100, 1);
        // Each message: 400 chars / 4 = 100 tokens
        let body = "x".repeat(400);
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];

        let report = transformer.transform(&mut messages, false);
        assert!(report.is_some(), "should report compaction");
        let report = report.unwrap();
        assert_eq!(report.dropped_count, 2);
        assert_eq!(report.tokens_before, 400);
        assert!(report.tokens_after < report.tokens_before);
        assert!(!report.overflow);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn sliding_window_transformer_no_report_under_budget() {
        let transformer = SlidingWindowTransformer::new(10_000, 5_000, 1);
        let mut messages = vec![text_message("hello"), text_message("world")];

        let report = transformer.transform(&mut messages, false);
        assert!(report.is_none(), "should not report when under budget");
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn closure_blanket_impl_works() {
        let closure = |msgs: &mut Vec<AgentMessage>, _overflow: bool| {
            if msgs.len() > 2 {
                msgs.truncate(2);
            }
        };

        let mut messages = vec![
            text_message("a"),
            text_message("b"),
            text_message("c"),
            text_message("d"),
        ];

        let report = closure.transform(&mut messages, false);
        assert!(report.is_some());
        let report = report.unwrap();
        assert_eq!(report.dropped_count, 2);
        // Bare closures can't report token counts
        assert_eq!(report.tokens_before, 0);
        assert_eq!(report.tokens_after, 0);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn overflow_uses_smaller_budget() {
        let transformer = SlidingWindowTransformer::new(1000, 150, 1);
        let body = "x".repeat(400);
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];

        // Under normal budget (1000), total is 400 tokens -- no trim.
        let report = transformer.transform(&mut messages, false);
        assert!(report.is_none());
        assert_eq!(messages.len(), 4);

        // Under overflow budget (150), should trim.
        let report = transformer.transform(&mut messages, true);
        assert!(report.is_some());
        let report = report.unwrap();
        assert!(report.overflow);
        assert!(messages.len() < 4);
    }
}
