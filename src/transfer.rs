//! Transfer types and tool for agent-to-agent handoff signaling.
//!
//! This module provides [`TransferSignal`], [`TransferChain`], [`TransferError`],
//! and the [`TransferToAgentTool`] that signals the agent loop to transfer
//! conversation to another agent.

use std::collections::HashSet;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::registry::AgentRegistry;
use crate::tool::{AgentTool, AgentToolResult, ToolFuture, validated_schema_for};
use crate::types::LlmMessage;

// ─── TransferSignal ────────────────────────────────────────────────────────

/// Data structure carrying all information needed for a target agent to
/// continue a conversation after a handoff.
///
/// Created by the transfer tool with target, reason, and optional summary.
/// The agent loop enriches it with `conversation_history` before surfacing
/// it in [`AgentResult`](crate::AgentResult).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferSignal {
    target_agent: String,
    reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_summary: Option<String>,
    #[serde(default)]
    conversation_history: Vec<LlmMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transfer_chain: Option<TransferChain>,
}

impl TransferSignal {
    /// Create a new transfer signal with a target agent and reason.
    pub fn new(target_agent: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            target_agent: target_agent.into(),
            reason: reason.into(),
            context_summary: None,
            conversation_history: Vec::new(),
            transfer_chain: None,
        }
    }

    /// Set an optional context summary for the target agent.
    #[must_use]
    pub fn with_context_summary(mut self, summary: impl Into<String>) -> Self {
        self.context_summary = Some(summary.into());
        self
    }

    /// Set the conversation history to carry over to the target agent.
    ///
    /// Only LLM messages are included; custom messages are filtered out
    /// by the agent loop before setting this field.
    #[must_use]
    pub fn with_conversation_history(mut self, history: Vec<LlmMessage>) -> Self {
        self.conversation_history = history;
        self
    }

    /// Set the transfer chain to carry across agent handoffs.
    ///
    /// The receiving agent can seed its loop with this chain so circular and
    /// max-depth checks continue across transfers.
    #[must_use]
    pub fn with_transfer_chain(mut self, chain: TransferChain) -> Self {
        self.transfer_chain = Some(chain);
        self
    }

    /// The name of the agent to transfer to.
    pub fn target_agent(&self) -> &str {
        &self.target_agent
    }

    /// The reason for the transfer.
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Optional concise handoff brief for the target agent.
    pub fn context_summary(&self) -> Option<&str> {
        self.context_summary.as_deref()
    }

    /// Messages to carry over to the target agent (LLM messages only).
    pub fn conversation_history(&self) -> &[LlmMessage] {
        &self.conversation_history
    }

    /// Transfer chain captured at handoff time, if present.
    pub const fn transfer_chain(&self) -> Option<&TransferChain> {
        self.transfer_chain.as_ref()
    }
}

// ─── TransferError ─────────────────────────────────────────────────────────

/// Error type for transfer chain safety violations.
#[non_exhaustive]
#[derive(Debug, Clone, thiserror::Error)]
pub enum TransferError {
    /// Agent already appears in the transfer chain (circular reference).
    #[error("circular transfer detected: agent '{agent_name}' already in chain {chain:?}")]
    CircularTransfer {
        agent_name: String,
        chain: Vec<String>,
    },
    /// Transfer chain would exceed the configured maximum depth.
    #[error("max transfer depth exceeded: depth {depth} >= max {max}")]
    MaxDepthExceeded { depth: usize, max: usize },
}

// ─── TransferChain ─────────────────────────────────────────────────────────

/// Safety mechanism tracking the ordered sequence of agents in a transfer chain.
///
/// The orchestrator creates a new chain per user message and carries it forward
/// through transfers. This prevents infinite handoff loops and enforces depth limits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferChain {
    chain: Vec<String>,
    max_depth: usize,
}

impl TransferChain {
    /// Create a new empty chain with the given maximum depth.
    pub const fn new(max_depth: usize) -> Self {
        Self {
            chain: Vec::new(),
            max_depth,
        }
    }

    /// Push an agent onto the chain.
    ///
    /// Returns `Err(TransferError::CircularTransfer)` if the agent is already in the chain.
    /// Returns `Err(TransferError::MaxDepthExceeded)` if the chain is at max depth.
    pub fn push(&mut self, agent_name: impl Into<String>) -> Result<(), TransferError> {
        let name = agent_name.into();
        if self.chain.contains(&name) {
            return Err(TransferError::CircularTransfer {
                agent_name: name,
                chain: self.chain.clone(),
            });
        }
        if self.chain.len() >= self.max_depth {
            return Err(TransferError::MaxDepthExceeded {
                depth: self.chain.len(),
                max: self.max_depth,
            });
        }
        self.chain.push(name);
        Ok(())
    }

    /// Current depth of the chain (number of agents).
    pub const fn depth(&self) -> usize {
        self.chain.len()
    }

    /// Check if an agent is already in the chain.
    pub fn contains(&self, agent_name: &str) -> bool {
        self.chain.iter().any(|n| n == agent_name)
    }

    /// The ordered list of agent names in this chain.
    pub fn chain(&self) -> &[String] {
        &self.chain
    }
}

impl Default for TransferChain {
    fn default() -> Self {
        Self::new(5)
    }
}

// ─── TransferToAgentTool ───────────────────────────────────────────────────

/// Parameters accepted by [`TransferToAgentTool`].
#[derive(Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
struct TransferParams {
    /// Name of the agent to transfer to.
    agent_name: String,
    /// Why the transfer is needed.
    reason: String,
    /// Optional summary for the target agent.
    context_summary: Option<String>,
}

/// Tool that signals the agent loop to transfer conversation to another agent.
///
/// When called, validates the target exists in the [`AgentRegistry`] (and
/// optionally that it appears in the allowed-targets set), then returns an
/// [`AgentToolResult`] carrying a [`TransferSignal`]. The agent loop detects
/// the signal and terminates the turn with
/// [`StopReason::Transfer`](crate::StopReason::Transfer).
pub struct TransferToAgentTool {
    registry: Arc<AgentRegistry>,
    allowed_targets: Option<HashSet<String>>,
    schema: Value,
}

// Constructors only used externally via `pub use` under `feature = "transfer"`;
// integration tests don't count for lib's dead_code analysis.
#[allow(dead_code)]
impl TransferToAgentTool {
    /// Create a new transfer tool that can transfer to any registered agent.
    pub fn new(registry: Arc<AgentRegistry>) -> Self {
        Self {
            registry,
            allowed_targets: None,
            schema: validated_schema_for::<TransferParams>(),
        }
    }

    /// Create a transfer tool restricted to the given set of allowed target agents.
    pub fn with_allowed_targets(
        registry: Arc<AgentRegistry>,
        targets: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            registry,
            allowed_targets: Some(targets.into_iter().map(Into::into).collect()),
            schema: validated_schema_for::<TransferParams>(),
        }
    }
}

impl AgentTool for TransferToAgentTool {
    #[allow(clippy::unnecessary_literal_bound)]
    fn name(&self) -> &str {
        "transfer_to_agent"
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn label(&self) -> &str {
        "Transfer to Agent"
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn description(&self) -> &str {
        "Transfer the conversation to another agent. Use when the user's request \
         is better handled by a different specialist agent."
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<crate::SessionState>>,
        _credential: Option<crate::credential::ResolvedCredential>,
    ) -> ToolFuture<'_> {
        Box::pin(async move {
            let parsed: TransferParams = match serde_json::from_value(params) {
                Ok(p) => p,
                Err(e) => return AgentToolResult::error(format!("invalid parameters: {e}")),
            };

            if cancellation_token.is_cancelled() {
                return AgentToolResult::error("cancelled");
            }

            // Check allowed targets if restricted
            if let Some(ref allowed) = self.allowed_targets
                && !allowed.contains(&parsed.agent_name)
            {
                let mut sorted: Vec<&String> = allowed.iter().collect();
                sorted.sort();
                return AgentToolResult::error(format!(
                    "transfer to '{}' not allowed. Allowed targets: {sorted:?}",
                    parsed.agent_name,
                ));
            }

            // Validate target exists in registry
            if self.registry.get(&parsed.agent_name).is_none() {
                return AgentToolResult::error(format!(
                    "agent '{}' not found in registry",
                    parsed.agent_name
                ));
            }

            // Build transfer signal (partial — loop will enrich with history)
            let mut signal = TransferSignal::new(&parsed.agent_name, &parsed.reason);
            if let Some(summary) = parsed.context_summary {
                signal = signal.with_context_summary(summary);
            }

            AgentToolResult::transfer(signal)
        })
    }
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<TransferSignal>();
    assert_send_sync::<TransferChain>();
    assert_send_sync::<TransferError>();
    assert_send_sync::<TransferToAgentTool>();
};

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
