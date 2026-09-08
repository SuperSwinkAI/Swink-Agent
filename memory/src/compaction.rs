//! Summarization-aware context compaction.
//!
//! Wraps the core sliding-window strategy with the ability to inject a
//! pre-computed summary of dropped messages. The summary is generated
//! asynchronously (outside the agent loop) and stored for the next
//! synchronous compaction pass.

use std::sync::{Arc, Mutex};

#[allow(deprecated)]
use swink_agent::sliding_window;
use swink_agent::{AgentMessage, AssistantMessage, ContentBlock, LlmMessage};

/// Result of a compaction operation (diagnostic type for future use).
#[non_exhaustive]
#[derive(Debug)]
pub struct CompactionResult {
    /// The compacted messages.
    pub messages: Vec<AgentMessage>,
    /// Number of messages removed during compaction.
    pub removed_count: usize,
    /// The summary that was injected, if any.
    pub summary: Option<String>,
}

impl CompactionResult {
    /// Creates a new compaction result from its constituent parts.
    #[must_use]
    pub fn new(messages: Vec<AgentMessage>, removed_count: usize, summary: Option<String>) -> Self {
        Self {
            messages,
            removed_count,
            summary,
        }
    }
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
        #[allow(deprecated)]
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
                    let summary_msg = AgentMessage::Llm(LlmMessage::Assistant(
                        AssistantMessage::new(
                            vec![ContentBlock::Text {
                                text: format!("[Context summary of earlier conversation]\n{text}"),
                            }],
                            String::new(),
                            String::new(),
                        )
                        .with_timestamp(0),
                    ));

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
#[path = "compaction_tests.rs"]
mod tests;
