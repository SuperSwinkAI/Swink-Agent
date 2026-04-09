//! Public event types emitted by the agent loop.

use std::sync::Arc;

use crate::stream::AssistantMessageDelta;
use crate::tool::AgentToolResult;
use crate::types::{AgentMessage, AssistantMessage, LlmMessage, ModelSpec, ToolResultMessage};

// ─── TurnEndReason ───────────────────────────────────────────────────────────

/// Why a turn ended.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEndReason {
    /// Assistant completed without requesting tool calls.
    Complete,
    /// Tools were executed (loop continues).
    ToolsExecuted,
    /// Turn was interrupted by a steering message during tool execution.
    SteeringInterrupt,
    /// LLM returned an error stop reason.
    Error,
    /// External cancellation via `CancellationToken`.
    Cancelled,
    /// Stream was aborted mid-generation.
    Aborted,
    /// Turn ended due to a transfer signal from a tool.
    Transfer,
}

// ─── AgentEvent ──────────────────────────────────────────────────────────────

/// Fine-grained lifecycle event emitted by the agent loop.
///
/// Consumers subscribe to these events for observability, UI updates, and
/// logging. The harness never calls back into application logic for display
/// concerns.
#[non_exhaustive]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum AgentEvent {
    /// Emitted once when the loop begins.
    AgentStart,

    /// Emitted once when the loop exits, carrying the final message context.
    AgentEnd { messages: Arc<Vec<AgentMessage>> },

    /// Emitted at the beginning of each assistant turn.
    TurnStart,

    /// Emitted at the end of each turn with the assistant message and tool results.
    TurnEnd {
        assistant_message: AssistantMessage,
        tool_results: Vec<ToolResultMessage>,
        reason: TurnEndReason,
        /// Full context snapshot at the turn boundary for replay/auditing.
        snapshot: crate::types::TurnSnapshot,
    },

    /// Emitted after context transform, before the LLM streaming call.
    /// Allows plugins to observe/log the final prompt.
    BeforeLlmCall {
        system_prompt: String,
        messages: Vec<LlmMessage>,
        model: ModelSpec,
    },

    /// Emitted when a message begins streaming.
    MessageStart,

    /// Emitted for each incremental delta during assistant streaming.
    MessageUpdate { delta: AssistantMessageDelta },

    /// Emitted when a message is complete.
    MessageEnd { message: AssistantMessage },

    /// Emitted when a tool call begins execution.
    ToolExecutionStart {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },

    /// Emitted for intermediate partial results from a streaming tool.
    ToolExecutionUpdate { partial: AgentToolResult },

    /// Emitted when a tool call is pending approval.
    ToolApprovalRequested {
        id: String,
        name: String,
        arguments: serde_json::Value,
    },

    /// Emitted when a tool call approval decision is made.
    ToolApprovalResolved {
        id: String,
        name: String,
        approved: bool,
    },

    /// Emitted when a tool call completes.
    ToolExecutionEnd {
        id: String,
        /// The tool name (matches `ToolExecutionStart.name`, including any
        /// plugin namespace prefix) so observers can correlate end events
        /// with the tool that produced them.
        name: String,
        result: AgentToolResult,
        is_error: bool,
    },

    /// Emitted when context compaction drops messages.
    ContextCompacted {
        report: crate::context::CompactionReport,
    },

    /// Emitted when the agent falls back to a different model after exhausting
    /// retries on the current one.
    ModelFallback {
        from_model: ModelSpec,
        to_model: ModelSpec,
    },

    /// Emitted when the agent switches to a different model during a retry cycle.
    ModelCycled {
        old: ModelSpec,
        new: ModelSpec,
        reason: String,
    },

    /// Emitted when session state delta is flushed (non-empty only).
    /// Fired immediately before `TurnEnd`.
    StateChanged { delta: crate::StateDelta },

    /// Emitted when context caching acts on a turn (write or read).
    CacheAction {
        hint: crate::context_cache::CacheHint,
        prefix_tokens: usize,
    },

    /// Emitted when an MCP server connects successfully.
    McpServerConnected { server_name: String },

    /// Emitted when an MCP server disconnects.
    McpServerDisconnected { server_name: String, reason: String },

    /// Emitted when tools are discovered from an MCP server.
    McpToolsDiscovered {
        server_name: String,
        tool_count: usize,
    },

    /// Emitted when an MCP tool call begins execution.
    McpToolCallStarted {
        server_name: String,
        tool_name: String,
    },

    /// Emitted when an MCP tool call completes.
    McpToolCallCompleted {
        server_name: String,
        tool_name: String,
        is_error: bool,
    },

    /// Emitted when an artifact version is successfully saved.
    #[cfg(feature = "artifact-store")]
    ArtifactSaved {
        /// The session this artifact belongs to.
        session_id: String,
        /// The artifact name.
        name: String,
        /// The version number that was saved.
        version: u32,
    },

    /// Emitted when a tool signals a transfer to another agent.
    ///
    /// Contains the enriched [`TransferSignal`](crate::transfer::TransferSignal)
    /// with conversation history. Emitted immediately before the `TurnEnd` event
    /// with `TurnEndReason::Transfer`.
    TransferInitiated {
        signal: crate::transfer::TransferSignal,
    },

    /// A custom event emitted via [`Agent::emit`](crate::Agent::emit).
    Custom(crate::emit::Emission),
}
