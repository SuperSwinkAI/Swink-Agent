//! Internal type definitions shared across loop submodules.

use crate::types::{AgentMessage, AssistantMessage, LlmMessage, ToolResultMessage};

// ─── Type Aliases ────────────────────────────────────────────────────────────

/// Converts an `AgentMessage` to an optional `LlmMessage` for the provider.
pub type ConvertToLlmFn = dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync;

// ─── LoopState ──────────────────────────────────────────────────────────────

/// Mutable state threaded through the loop iterations.
pub struct LoopState {
    pub context_messages: Vec<AgentMessage>,
    pub pending_messages: Vec<AgentMessage>,
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
    /// The stream was interrupted mid-generation by a steering message.
    ///
    /// `MessageEnd` with partial content was already emitted. The turn handler
    /// should poll the steering queue and restart the turn so the agent
    /// processes the steering message immediately.
    SteeringInterrupt,
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
