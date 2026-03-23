//! Summarization-aware context compaction.
//!
//! Wraps the core sliding-window strategy with the ability to inject a
//! pre-computed summary of dropped messages. The summary is generated
//! asynchronously (outside the agent loop) and stored for the next
//! synchronous compaction pass.

use std::sync::{Arc, Mutex};

use swink_agent::{
    AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason, Usage,
    sliding_window,
};

/// Result of a compaction operation (diagnostic type for future use).
#[derive(Debug)]
pub struct CompactionResult {
    /// The compacted messages.
    pub messages: Vec<AgentMessage>,
    /// Number of messages removed during compaction.
    pub removed_count: usize,
    /// The summary that was injected, if any.
    pub summary: Option<String>,
}

/// Summarization-aware context compactor.
///
/// Combines the core [`sliding_window`] strategy with an optional summary
/// that replaces dropped messages. The summary is stored externally via
/// [`set_summary`](Self::set_summary) and injected after the anchor
/// messages during the next compaction pass.
///
/// # Usage
///
/// ```rust,ignore
/// let compactor = SummarizingCompactor::new(100_000, 50_000, 2);
/// let agent = Agent::new()
///     .with_transform_context(compactor.compaction_fn());
///
/// // After each turn, if messages were dropped:
/// compactor.set_summary("Summary of earlier conversation...");
/// ```
pub struct SummarizingCompactor {
    /// Pre-computed summary to inject during compaction.
    summary: Arc<Mutex<Option<String>>>,
    /// Normal token budget.
    normal_budget: usize,
    /// Overflow token budget.
    overflow_budget: usize,
    /// Number of anchor messages to preserve.
    anchor: usize,
}

impl SummarizingCompactor {
    /// Create a new compactor with the given budget parameters.
    pub fn new(normal_budget: usize, overflow_budget: usize, anchor: usize) -> Self {
        Self {
            summary: Arc::new(Mutex::new(None)),
            normal_budget,
            overflow_budget,
            anchor,
        }
    }

    /// Returns a closure compatible with `Agent::with_transform_context()`.
    ///
    /// Behaves like [`sliding_window`] but injects a stored summary after
    /// the anchor messages when compaction occurs. If no summary is stored,
    /// behaves identically to `sliding_window`.
    ///
    /// The summary is consumed after injection — it will not be re-injected
    /// on subsequent compaction passes.
    pub fn compaction_fn(&self) -> impl Fn(&mut Vec<AgentMessage>, bool) + Send + Sync {
        let summary = Arc::clone(&self.summary);
        let base = sliding_window(self.normal_budget, self.overflow_budget, self.anchor);
        let anchor = self.anchor;

        move |messages: &mut Vec<AgentMessage>, overflow: bool| {
            let len_before = messages.len();

            // Run the base sliding window compaction.
            base(messages, overflow);

            let len_after = messages.len();

            // If messages were dropped and we have a stored summary, inject it.
            if len_after < len_before {
                let mut guard = summary
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(text) = guard.take() {
                    let summary_msg = AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
                        content: vec![ContentBlock::Text {
                            text: format!("[Context summary of earlier conversation]\n{text}"),
                        }],
                        provider: String::new(),
                        model_id: String::new(),
                        usage: Usage::default(),
                        cost: Cost::default(),
                        stop_reason: StopReason::Stop,
                        error_message: None,
                        timestamp: 0,
                    }));

                    // Insert after anchor messages.
                    let insert_pos = anchor.min(messages.len());
                    messages.insert(insert_pos, summary_msg);
                }
            }
        }
    }

    /// Store a summary to be injected on the next compaction pass.
    ///
    /// This should be called after each turn where context was compacted,
    /// passing a summary of the messages that were dropped. In the future
    /// this will be generated via an LLM call; for now callers provide the
    /// text directly.
    pub fn set_summary(&self, text: impl Into<String>) {
        let mut guard = self
            .summary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = Some(text.into());
    }

    /// Clear the stored summary.
    pub fn clear_summary(&self) {
        let mut guard = self
            .summary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = None;
    }

    /// Returns true if a summary is currently stored.
    pub fn has_summary(&self) -> bool {
        let guard = self
            .summary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swink_agent::{ContentBlock, LlmMessage, UserMessage};

    fn text_message(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            timestamp: 0,
        }))
    }

    #[test]
    fn without_summary_behaves_like_sliding_window() {
        let compactor = SummarizingCompactor::new(250, 100, 1);
        let compact = compactor.compaction_fn();

        // Each message: 400 chars / 4 = 100 tokens
        let body = "x".repeat(400);
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];

        compact(&mut messages, false);

        // Same behavior as sliding_window(250, 100, 1): anchor(1) + tail(1) = 2
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn with_summary_injects_after_anchor() {
        let compactor = SummarizingCompactor::new(250, 100, 1);
        compactor.set_summary("Earlier we discussed testing strategies.");
        let compact = compactor.compaction_fn();

        let body = "x".repeat(400);
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];

        compact(&mut messages, false);

        // anchor(1) + summary(1) + tail(1) = 3
        assert_eq!(messages.len(), 3);

        // Second message should be the summary (after anchor).
        if let AgentMessage::Llm(LlmMessage::Assistant(a)) = &messages[1] {
            let text = ContentBlock::extract_text(&a.content);
            assert!(
                text.contains("[Context summary"),
                "expected summary prefix, got: {text}"
            );
            assert!(text.contains("testing strategies"));
        } else {
            panic!("expected assistant message at index 1");
        }
    }

    #[test]
    fn no_compaction_needed_no_summary_injected() {
        let compactor = SummarizingCompactor::new(10_000, 5_000, 1);
        compactor.set_summary("This should not appear.");
        let compact = compactor.compaction_fn();

        let mut messages = vec![text_message("hello"), text_message("world")];
        compact(&mut messages, false);

        // Under budget, no compaction, no summary injection.
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn clear_summary_removes_stored_text() {
        let compactor = SummarizingCompactor::new(100, 50, 1);
        compactor.set_summary("some summary");
        assert!(compactor.has_summary());

        compactor.clear_summary();
        assert!(!compactor.has_summary());
    }

    #[test]
    fn summary_injected_as_assistant_message() {
        let compactor = SummarizingCompactor::new(250, 100, 1);
        compactor.set_summary("Key discussion points here.");
        let compact = compactor.compaction_fn();

        let body = "x".repeat(400);
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];

        compact(&mut messages, false);

        // Find the summary message
        let summary_msg = messages.iter().find(|m| {
            if let AgentMessage::Llm(LlmMessage::Assistant(a)) = m {
                ContentBlock::extract_text(&a.content).contains("[Context summary")
            } else {
                false
            }
        });
        assert!(
            summary_msg.is_some(),
            "summary should be an AssistantMessage"
        );
    }

    #[test]
    fn compaction_with_single_message_returns_unchanged() {
        let compactor = SummarizingCompactor::new(10_000, 5_000, 1);
        let compact = compactor.compaction_fn();

        let mut messages = vec![text_message("single message")];
        compact(&mut messages, false);

        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn summary_consumed_after_injection() {
        let compactor = SummarizingCompactor::new(250, 100, 1);
        compactor.set_summary("Consumed summary.");
        let compact = compactor.compaction_fn();

        let body = "x".repeat(400);
        let mut messages = vec![
            text_message(&body),
            text_message(&body),
            text_message(&body),
            text_message(&body),
        ];

        compact(&mut messages, false);

        // Summary should have been consumed
        assert!(
            !compactor.has_summary(),
            "summary should be consumed after injection"
        );
    }
}
