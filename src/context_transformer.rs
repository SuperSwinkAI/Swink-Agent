//! Pluggable context transformation with compaction awareness.
//!
//! Replaces the bare `TransformContextFn` closure with a trait that supports
//! both transformation and compaction reporting.

use std::sync::Arc;

use crate::context::{CompactionReport, TokenCounter, compact_sliding_window_with};
use crate::types::AgentMessage;

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
                dropped_messages: Vec::new(), // bare closures don't have access to the dropped slice
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
///
/// Accepts an optional [`TokenCounter`] for pluggable token estimation.
/// When none is provided, the default `chars / 4` heuristic is used.
pub struct SlidingWindowTransformer {
    normal_budget: usize,
    overflow_budget: usize,
    anchor: usize,
    token_counter: Option<Arc<dyn TokenCounter>>,
    /// When caching is active, protects this many messages from compaction.
    /// The effective anchor becomes `max(anchor, cached_prefix_len)`.
    cached_prefix_len: usize,
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
    pub fn new(normal_budget: usize, overflow_budget: usize, anchor: usize) -> Self {
        Self {
            normal_budget,
            overflow_budget,
            anchor,
            token_counter: None,
            cached_prefix_len: 0,
        }
    }

    #[must_use]
    pub fn with_token_counter(mut self, counter: Arc<dyn TokenCounter>) -> Self {
        self.token_counter = Some(counter);
        self
    }

    /// Set the cached prefix length to protect from compaction.
    ///
    /// When caching is active, the effective anchor is `max(anchor, cached_prefix_len)`.
    #[must_use]
    pub const fn with_cached_prefix_len(mut self, len: usize) -> Self {
        self.cached_prefix_len = len;
        self
    }

    /// Update the cached prefix length (for runtime updates from the turn pipeline).
    pub const fn set_cached_prefix_len(&mut self, len: usize) {
        self.cached_prefix_len = len;
    }
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

        let effective_anchor = self.anchor.max(self.cached_prefix_len);
        let counter_ref = self.token_counter.as_deref();
        let mut report =
            compact_sliding_window_with(messages, budget, effective_anchor, counter_ref)?;
        report.overflow = overflow;
        Some(report)
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
            cache_hint: None,
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

    #[test]
    fn sliding_window_transformer_with_custom_counter() {
        use crate::context::TokenCounter;

        /// Counts every character as one token (4x the default heuristic).
        struct CharCounter;

        impl TokenCounter for CharCounter {
            fn count_tokens(&self, message: &AgentMessage) -> usize {
                match message {
                    AgentMessage::Llm(llm) => {
                        let blocks = match llm {
                            LlmMessage::User(m) => &m.content,
                            _ => return 0,
                        };
                        blocks
                            .iter()
                            .map(|b| match b {
                                ContentBlock::Text { text } => text.len(),
                                _ => 0,
                            })
                            .sum()
                    }
                    AgentMessage::Custom(_) => 50,
                }
            }
        }

        // Each message: 400 chars.
        // Default counter: 400/4 = 100 tokens each.
        // CharCounter: 400 tokens each.
        let body = "x".repeat(400);

        // With default counter, 4 * 100 = 400 tokens. Budget 500 => no trim.
        let default_transformer = SlidingWindowTransformer::new(500, 250, 1);
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];
        let report = default_transformer.transform(&mut messages, false);
        assert!(
            report.is_none(),
            "default counter should not trim at budget 500"
        );
        assert_eq!(messages.len(), 4);

        // With CharCounter, 4 * 400 = 1600 tokens. Budget 500 => trims.
        let custom_transformer =
            SlidingWindowTransformer::new(500, 250, 1).with_token_counter(Arc::new(CharCounter));
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];
        let report = custom_transformer.transform(&mut messages, false);
        assert!(report.is_some(), "char counter should trim at budget 500");
        assert!(messages.len() < 4);
    }
}
