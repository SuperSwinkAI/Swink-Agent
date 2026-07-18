//! Internal type definitions shared across loop submodules.

use std::ops::Deref;
use std::sync::Arc;

use crate::types::{AgentMessage, AssistantMessage, LlmMessage, ToolResultMessage};

// ─── Type Aliases ────────────────────────────────────────────────────────────

/// Converts an `AgentMessage` to an optional `LlmMessage` for the provider.
pub type ConvertToLlmFn = dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync;

// ─── ContextMessages ────────────────────────────────────────────────────────

/// The loop's conversation history with an `Arc`-shared mirror of its LLM
/// messages, so [`TurnSnapshot`](crate::types::TurnSnapshot) construction at
/// every turn boundary is amortized O(messages touched since the last
/// snapshot) instead of deep-cloning the entire history each turn.
///
/// # Invariants
///
/// - `mirror.len() <= messages.len()` at all times.
/// - For every `i < mirror.len()`: `mirror[i]` is a content-equal `Arc` copy
///   of `messages[i]`'s LLM payload when `messages[i]` is `Llm`, and `None`
///   when it is `Custom`.
///
/// All mutation goes through methods that preserve those invariants:
/// append-only operations leave the mirrored prefix valid, targeted
/// replacement ([`Self::set`]) truncates the mirror at the replaced index,
/// and bulk in-place mutation ([`Self::make_mut`]) discards the mirror
/// entirely. [`Self::snapshot_llm`] then re-clones only the messages beyond
/// the still-valid mirror prefix.
pub struct ContextMessages {
    messages: Vec<AgentMessage>,
    /// `Arc` mirror aligned 1:1 with the leading `mirror.len()` entries of
    /// `messages` (`None` for `Custom` entries). Lazily extended by
    /// [`Self::snapshot_llm`].
    mirror: Vec<Option<Arc<LlmMessage>>>,
}

impl ContextMessages {
    /// Wrap an owned message history.
    #[must_use]
    pub fn new(messages: Vec<AgentMessage>) -> Self {
        Self {
            messages,
            mirror: Vec::new(),
        }
    }

    /// Append a single message.
    pub fn push(&mut self, message: AgentMessage) {
        self.messages.push(message);
    }

    /// Append every message from `iter`.
    pub fn extend<I: IntoIterator<Item = AgentMessage>>(&mut self, iter: I) {
        self.messages.extend(iter);
    }

    /// Move all messages out of `other`, appending them to the history.
    pub fn append(&mut self, other: &mut Vec<AgentMessage>) {
        self.messages.append(other);
    }

    /// Replace the message at `index`, invalidating the mirrored prefix from
    /// that index on so the next snapshot re-clones the replaced tail.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds, like `Vec` indexing.
    pub fn set(&mut self, index: usize, message: AgentMessage) {
        self.messages[index] = message;
        if index < self.mirror.len() {
            self.mirror.truncate(index);
        }
    }

    /// Mutable access to the underlying vector for bulk in-place mutation
    /// (context transformers, cache-hint annotation, tool-result rewrites).
    ///
    /// Discards the whole mirror, so the next [`Self::snapshot_llm`] deep
    /// clones the full history again — prefer the targeted methods above on
    /// hot paths.
    pub fn make_mut(&mut self) -> &mut Vec<AgentMessage> {
        self.mirror.clear();
        &mut self.messages
    }

    /// Take the underlying vector, leaving the history empty.
    pub fn take_vec(&mut self) -> Vec<AgentMessage> {
        self.mirror.clear();
        std::mem::take(&mut self.messages)
    }

    /// Consume the history into the underlying vector.
    #[must_use]
    pub fn into_vec(self) -> Vec<AgentMessage> {
        self.messages
    }

    /// View the history as a message slice.
    #[must_use]
    pub fn as_slice(&self) -> &[AgentMessage] {
        &self.messages
    }

    /// Build an `Arc`-shared snapshot of the LLM messages in the history.
    ///
    /// Messages already mirrored by a previous snapshot are reused as cheap
    /// `Arc` clones (pointer bumps); only messages appended or touched since
    /// then are deep-cloned into fresh `Arc`s.
    pub fn snapshot_llm(&mut self) -> Arc<Vec<Arc<LlmMessage>>> {
        let start = self.mirror.len();
        for message in &self.messages[start..] {
            self.mirror.push(match message {
                AgentMessage::Llm(llm) => Some(Arc::new(llm.clone())),
                AgentMessage::Custom(_) => None,
            });
        }
        Arc::new(self.mirror.iter().flatten().cloned().collect())
    }
}

impl Deref for ContextMessages {
    type Target = [AgentMessage];

    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}

// ─── LoopState ──────────────────────────────────────────────────────────────

/// Mutable state threaded through the loop iterations.
pub struct LoopState {
    pub context_messages: ContextMessages,
    pub pending_messages: Vec<AgentMessage>,
    /// Initial prompt messages or resumed pending messages that should be
    /// exposed once as the first pre-turn `new_messages` batch.
    pub initial_new_messages_len: usize,
    pub overflow_signal: bool,
    /// Whether emergency overflow recovery has already been attempted this turn.
    /// Resets to `false` at the start of each turn.
    pub overflow_recovery_attempted: bool,
    pub turn_index: usize,
    pub accumulated_usage: crate::types::Usage,
    pub accumulated_cost: crate::types::Cost,
    /// The last assistant message from a completed turn (for policy checks).
    pub last_assistant_message: Option<AssistantMessage>,
    /// Tool results from the last completed turn (for post-turn hook).
    pub last_tool_results: Vec<ToolResultMessage>,
    /// Transfer chain tracking agent handoff sequence for safety enforcement.
    /// Prevents circular transfers and enforces max-depth limits.
    pub transfer_chain: crate::transfer::TransferChain,
}

// ─── TurnOutcome ────────────────────────────────────────────────────────────

/// Outcome of a single turn execution within the inner loop.
pub enum TurnOutcome {
    /// Continue to the next inner-loop iteration (tool results need processing).
    ContinueInner,
    /// Break out of the inner loop (no tool calls, check follow-ups).
    BreakInner,
    /// Return from the entire loop (channel closed, error, or abort).
    Return,
}

// ─── ToolCallInfo ───────────────────────────────────────────────────────────

/// Info about a tool call extracted from the assistant message.
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub is_incomplete: bool,
}

// ─── StreamResult ───────────────────────────────────────────────────────────

/// Result of streaming an assistant response.
#[allow(clippy::large_enum_variant)]
pub enum StreamResult {
    Message(AssistantMessage),
    ContextOverflow,
    Aborted,
    ChannelClosed,
}

// ─── ToolExecOutcome ────────────────────────────────────────────────────────

/// Outcome of concurrent tool execution.
pub enum ToolExecOutcome {
    Completed {
        results: Vec<ToolResultMessage>,
        tool_metrics: Vec<crate::metrics::ToolExecMetrics>,
        /// Transfer signal detected during tool execution (first one wins).
        transfer_signal: Option<crate::transfer::TransferSignal>,
        /// Messages injected by `PreDispatch` policies via `Inject` verdict,
        /// to be appended to `pending_messages` for the next turn.
        injected_messages: Vec<AgentMessage>,
    },
    Stopped {
        results: Vec<ToolResultMessage>,
        tool_metrics: Vec<crate::metrics::ToolExecMetrics>,
        /// Stop reason returned by the batch-wide pre-dispatch pass.
        reason: String,
        /// Messages injected by `PreDispatch` policies before the stop fired.
        injected_messages: Vec<AgentMessage>,
    },
    SteeringInterrupt {
        completed: Vec<ToolResultMessage>,
        cancelled: Vec<ToolResultMessage>,
        steering_messages: Vec<AgentMessage>,
        tool_metrics: Vec<crate::metrics::ToolExecMetrics>,
        /// Messages injected by `PreDispatch` policies before the steering interrupt.
        injected_messages: Vec<AgentMessage>,
    },
    Aborted {
        /// Deterministic results collected before the abort signal, plus
        /// synthetic cancellation results for unfinished tool calls.
        results: Vec<ToolResultMessage>,
        tool_metrics: Vec<crate::metrics::ToolExecMetrics>,
        /// Messages injected by `PreDispatch` policies before cancellation.
        injected_messages: Vec<AgentMessage>,
    },
    ChannelClosed,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::ContextMessages;
    use crate::types::{AgentMessage, ContentBlock, LlmMessage, UserMessage};

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))
    }

    fn text_of(message: &LlmMessage) -> &str {
        match message {
            LlmMessage::User(u) => match &u.content[0] {
                ContentBlock::Text { text } => text,
                other => panic!("expected text content, got {other:?}"),
            },
            other => panic!("expected user message, got {other:?}"),
        }
    }

    #[test]
    fn snapshot_reuses_arcs_for_untouched_prefix() {
        let mut context = ContextMessages::new(vec![user_msg("a"), user_msg("b")]);
        let first = context.snapshot_llm();

        context.push(user_msg("c"));
        let second = context.snapshot_llm();

        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 3);
        assert!(
            Arc::ptr_eq(&first[0], &second[0]) && Arc::ptr_eq(&first[1], &second[1]),
            "untouched prefix must be shared, not deep-copied"
        );
        assert_eq!(text_of(&second[2]), "c");
    }

    #[test]
    fn set_invalidates_mirror_from_replaced_index() {
        let mut context = ContextMessages::new(vec![user_msg("a"), user_msg("b"), user_msg("c")]);
        let first = context.snapshot_llm();

        context.set(1, user_msg("b2"));
        let second = context.snapshot_llm();

        assert!(
            Arc::ptr_eq(&first[0], &second[0]),
            "prefix before the replaced index stays shared"
        );
        assert!(
            !Arc::ptr_eq(&first[1], &second[1]),
            "the replaced message must be re-cloned"
        );
        assert_eq!(text_of(&second[1]), "b2");
        assert_eq!(text_of(&second[2]), "c");
    }

    #[test]
    fn make_mut_invalidates_whole_mirror_and_reflects_mutation() {
        let mut context = ContextMessages::new(vec![user_msg("a"), user_msg("b")]);
        let first = context.snapshot_llm();

        if let AgentMessage::Llm(LlmMessage::User(u)) = &mut context.make_mut()[0] {
            u.content = vec![ContentBlock::Text {
                text: "a2".to_string(),
            }];
        }
        let second = context.snapshot_llm();

        assert!(
            !Arc::ptr_eq(&first[0], &second[0]),
            "bulk mutation must invalidate the mirror"
        );
        assert_eq!(text_of(&second[0]), "a2");
        assert_eq!(text_of(&second[1]), "b");
    }

    #[test]
    fn take_vec_empties_history_and_mirror() {
        let mut context = ContextMessages::new(vec![user_msg("a")]);
        let _ = context.snapshot_llm();

        let taken = context.take_vec();

        assert_eq!(taken.len(), 1);
        assert!(context.as_slice().is_empty());
        assert!(context.snapshot_llm().is_empty());
    }
}
