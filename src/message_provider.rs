//! Trait for polling steering and follow-up messages.
//!
//! [`MessageProvider`] replaces inline closures in [`AgentLoopConfig`](crate::loop_::AgentLoopConfig),
//! giving callers a named, testable abstraction for injecting messages into the
//! agent loop between turns.

use crate::types::AgentMessage;

/// Provides steering and follow-up messages to the agent loop.
///
/// Implementors are polled at well-defined points during loop execution:
/// - [`poll_steering`](Self::poll_steering) is called after each tool execution batch.
/// - [`poll_follow_up`](Self::poll_follow_up) is called when the agent would otherwise stop.
pub trait MessageProvider: Send + Sync {
    /// Return pending steering messages, if any.
    ///
    /// Called after tool execution completes. Returning a non-empty vec causes
    /// a steering interrupt — pending tool calls may be cancelled and the new
    /// messages are injected into the conversation.
    fn poll_steering(&self) -> Vec<AgentMessage>;

    /// Return pending follow-up messages, if any.
    ///
    /// Called when the model has finished a turn and no tool calls remain.
    /// Returning a non-empty vec triggers another outer-loop iteration.
    fn poll_follow_up(&self) -> Vec<AgentMessage>;
}
