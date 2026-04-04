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
}

impl TransferSignal {
    /// Create a new transfer signal with a target agent and reason.
    pub fn new(target_agent: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            target_agent: target_agent.into(),
            reason: reason.into(),
            context_summary: None,
            conversation_history: Vec::new(),
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
}

// ─── TransferError ─────────────────────────────────────────────────────────

/// Error type for transfer chain safety violations.
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
#[derive(Debug, Clone)]
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
mod tests {
    use super::*;

    // T007: TransferSignal constructors, accessors, and serde round-trip

    #[test]
    fn transfer_signal_new_sets_target_and_reason() {
        let signal = TransferSignal::new("billing", "billing issue");
        assert_eq!(signal.target_agent(), "billing");
        assert_eq!(signal.reason(), "billing issue");
        assert_eq!(signal.context_summary(), None);
        assert!(signal.conversation_history().is_empty());
    }

    #[test]
    fn transfer_signal_with_context_summary() {
        let signal = TransferSignal::new("billing", "billing issue")
            .with_context_summary("User has a $50 charge they dispute");
        assert_eq!(
            signal.context_summary(),
            Some("User has a $50 charge they dispute")
        );
    }

    #[test]
    fn transfer_signal_with_conversation_history() {
        use crate::types::{ContentBlock, UserMessage};

        let msg = LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
            timestamp: 0,
            cache_hint: None,
        });
        let signal =
            TransferSignal::new("tech", "tech issue").with_conversation_history(vec![msg]);
        assert_eq!(signal.conversation_history().len(), 1);
    }

    #[test]
    fn transfer_signal_serde_roundtrip() {
        let signal = TransferSignal::new("billing", "billing issue")
            .with_context_summary("User disputes charge");
        let json = serde_json::to_string(&signal).unwrap();
        let parsed: TransferSignal = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.target_agent(), "billing");
        assert_eq!(parsed.reason(), "billing issue");
        assert_eq!(parsed.context_summary(), Some("User disputes charge"));
        assert!(parsed.conversation_history().is_empty());
    }

    #[test]
    fn transfer_signal_deserialize_without_optional_fields() {
        let json = r#"{"target_agent":"billing","reason":"billing issue"}"#;
        let parsed: TransferSignal = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.target_agent(), "billing");
        assert_eq!(parsed.reason(), "billing issue");
        assert_eq!(parsed.context_summary(), None);
        assert!(parsed.conversation_history().is_empty());
    }

    #[test]
    fn transfer_signal_serde_skips_none_context_summary() {
        let signal = TransferSignal::new("billing", "billing issue");
        let json = serde_json::to_value(&signal).unwrap();
        assert!(!json.as_object().unwrap().contains_key("context_summary"));
    }

    #[test]
    fn transfer_signal_builder_chain() {
        let signal = TransferSignal::new("target", "reason")
            .with_context_summary("summary")
            .with_conversation_history(vec![]);
        assert_eq!(signal.target_agent(), "target");
        assert_eq!(signal.reason(), "reason");
        assert_eq!(signal.context_summary(), Some("summary"));
        assert!(signal.conversation_history().is_empty());
    }

    // T025: TransferChain rejects circular transfer
    #[test]
    fn transfer_chain_rejects_circular() {
        let mut chain = TransferChain::default();
        chain.push("agent-a").unwrap();
        chain.push("agent-b").unwrap();
        let err = chain.push("agent-a").unwrap_err();
        assert!(matches!(err, TransferError::CircularTransfer { .. }));
    }

    // T026: TransferChain rejects when max_depth exceeded
    #[test]
    fn transfer_chain_rejects_max_depth() {
        let mut chain = TransferChain::new(2);
        chain.push("a").unwrap();
        chain.push("b").unwrap();
        let err = chain.push("c").unwrap_err();
        assert!(matches!(
            err,
            TransferError::MaxDepthExceeded { depth: 2, max: 2 }
        ));
    }

    // T027: TransferChain allows push of new agent
    #[test]
    fn transfer_chain_allows_new_agent() {
        let mut chain = TransferChain::default();
        assert!(chain.push("agent-a").is_ok());
        assert!(chain.push("agent-b").is_ok());
        assert!(chain.push("agent-c").is_ok());
    }

    // T028: TransferChain::default() has max_depth 5
    #[test]
    fn transfer_chain_default_max_depth() {
        let mut chain = TransferChain::default();
        // Push 5 agents, all should succeed
        for i in 0..5 {
            chain.push(format!("agent-{i}")).unwrap();
        }
        // 6th should fail
        let err = chain.push("agent-5").unwrap_err();
        assert!(matches!(err, TransferError::MaxDepthExceeded { .. }));
    }

    // T029: TransferChain::contains() and depth()
    #[test]
    fn transfer_chain_contains_and_depth() {
        let mut chain = TransferChain::default();
        assert_eq!(chain.depth(), 0);
        assert!(!chain.contains("a"));

        chain.push("a").unwrap();
        assert_eq!(chain.depth(), 1);
        assert!(chain.contains("a"));
        assert!(!chain.contains("b"));

        chain.push("b").unwrap();
        assert_eq!(chain.depth(), 2);
        assert!(chain.contains("b"));
        assert_eq!(chain.chain(), &["a", "b"]);
    }

    // T030: Self-transfer is always circular
    #[test]
    fn transfer_chain_self_transfer_is_circular() {
        let mut chain = TransferChain::default();
        chain.push("support").unwrap();
        // Trying to push the same agent that's already first (self-transfer)
        let err = chain.push("support").unwrap_err();
        assert!(
            matches!(err, TransferError::CircularTransfer { agent_name, .. } if agent_name == "support")
        );
    }

    // T036: TransferSignal has target, reason, context_summary
    #[test]
    fn transfer_signal_carries_full_context() {
        let signal = TransferSignal::new("billing", "billing question")
            .with_context_summary("User asked about invoice #123");
        assert_eq!(signal.target_agent(), "billing");
        assert_eq!(signal.reason(), "billing question");
        assert_eq!(
            signal.context_summary(),
            Some("User asked about invoice #123")
        );
    }

    // ── TransferToAgentTool tests (T011-T015) ────────────────────────────

    #[cfg(feature = "testkit")]
    mod transfer_tool_tests {
        use super::*;
        use crate::agent::{Agent, AgentOptions};
        use crate::registry::AgentRegistry;
        use crate::testing::SimpleMockStreamFn;
        use crate::tool::AgentTool;
        use crate::types::ModelSpec;
        use tokio_util::sync::CancellationToken;

        /// Build a minimal Agent suitable for registering in the registry.
        fn dummy_agent() -> Agent {
            Agent::new(AgentOptions::new(
                "test",
                ModelSpec::new("test", "test-model"),
                std::sync::Arc::new(SimpleMockStreamFn::from_text("hi")),
                crate::agent::default_convert,
            ))
        }

        // T011: TransferToAgentTool validates target exists, returns transfer signal
        #[tokio::test]
        async fn transfer_tool_validates_target_and_returns_signal() {
            let registry = std::sync::Arc::new(AgentRegistry::new());
            registry.register("billing", dummy_agent());

            let tool = TransferToAgentTool::new(registry);
            let params = serde_json::json!({
                "agent_name": "billing",
                "reason": "billing question"
            });

            let result = tool
                .execute(
                    "tc-1",
                    params,
                    CancellationToken::new(),
                    None,
                    std::sync::Arc::new(std::sync::RwLock::new(crate::SessionState::default())),
                    None,
                )
                .await;

            assert!(!result.is_error);
            assert!(result.is_transfer());
            let signal = result.transfer_signal.unwrap();
            assert_eq!(signal.target_agent(), "billing");
            assert_eq!(signal.reason(), "billing question");
            assert_eq!(signal.context_summary(), None);
            // History is empty — loop enriches it later
            assert!(signal.conversation_history().is_empty());
        }

        // T012: Target not in registry returns error
        #[tokio::test]
        async fn transfer_tool_target_not_found_returns_error() {
            let registry = std::sync::Arc::new(AgentRegistry::new());
            // Registry is empty — no agents registered

            let tool = TransferToAgentTool::new(registry);
            let params = serde_json::json!({
                "agent_name": "nonexistent",
                "reason": "test"
            });

            let result = tool
                .execute(
                    "tc-1",
                    params,
                    CancellationToken::new(),
                    None,
                    std::sync::Arc::new(std::sync::RwLock::new(crate::SessionState::default())),
                    None,
                )
                .await;

            assert!(result.is_error);
            assert!(!result.is_transfer());
            let text = &result.content[0];
            match text {
                crate::types::ContentBlock::Text { text } => {
                    assert!(
                        text.contains("not found in registry"),
                        "expected 'not found in registry', got: {text}"
                    );
                }
                _ => panic!("expected text content block"),
            }
        }

        // T013: context_summary included in signal when provided
        #[tokio::test]
        async fn transfer_tool_includes_context_summary() {
            let registry = std::sync::Arc::new(AgentRegistry::new());
            registry.register("billing", dummy_agent());

            let tool = TransferToAgentTool::new(registry);
            let params = serde_json::json!({
                "agent_name": "billing",
                "reason": "billing dispute",
                "context_summary": "User has a $50 charge they want to dispute"
            });

            let result = tool
                .execute(
                    "tc-1",
                    params,
                    CancellationToken::new(),
                    None,
                    std::sync::Arc::new(std::sync::RwLock::new(crate::SessionState::default())),
                    None,
                )
                .await;

            assert!(!result.is_error);
            let signal = result.transfer_signal.unwrap();
            assert_eq!(
                signal.context_summary(),
                Some("User has a $50 charge they want to dispute")
            );
        }

        // T015: Result text is "Transfer to {agent_name} initiated."
        #[tokio::test]
        async fn transfer_tool_result_text_format() {
            let registry = std::sync::Arc::new(AgentRegistry::new());
            registry.register("billing", dummy_agent());

            let tool = TransferToAgentTool::new(registry);
            let params = serde_json::json!({
                "agent_name": "billing",
                "reason": "billing question"
            });

            let result = tool
                .execute(
                    "tc-1",
                    params,
                    CancellationToken::new(),
                    None,
                    std::sync::Arc::new(std::sync::RwLock::new(crate::SessionState::default())),
                    None,
                )
                .await;

            let text = &result.content[0];
            match text {
                crate::types::ContentBlock::Text { text } => {
                    assert_eq!(text, "Transfer to billing initiated.");
                }
                _ => panic!("expected text content block"),
            }
        }

        // Additional: allowed_targets restricts transfers
        #[tokio::test]
        async fn transfer_tool_allowed_targets_restricts() {
            let registry = std::sync::Arc::new(AgentRegistry::new());
            registry.register("billing", dummy_agent());
            registry.register("tech", dummy_agent());

            // Only allow billing
            let tool = TransferToAgentTool::with_allowed_targets(
                registry,
                vec!["billing"],
            );
            let params = serde_json::json!({
                "agent_name": "tech",
                "reason": "tech question"
            });

            let result = tool
                .execute(
                    "tc-1",
                    params,
                    CancellationToken::new(),
                    None,
                    std::sync::Arc::new(std::sync::RwLock::new(crate::SessionState::default())),
                    None,
                )
                .await;

            assert!(result.is_error);
            let text = &result.content[0];
            match text {
                crate::types::ContentBlock::Text { text } => {
                    assert!(
                        text.contains("not allowed"),
                        "expected 'not allowed', got: {text}"
                    );
                }
                _ => panic!("expected text content block"),
            }
        }

        // Additional: allowed_targets permits valid target
        #[tokio::test]
        async fn transfer_tool_allowed_targets_permits() {
            let registry = std::sync::Arc::new(AgentRegistry::new());
            registry.register("billing", dummy_agent());

            let tool = TransferToAgentTool::with_allowed_targets(
                registry,
                vec!["billing"],
            );
            let params = serde_json::json!({
                "agent_name": "billing",
                "reason": "billing question"
            });

            let result = tool
                .execute(
                    "tc-1",
                    params,
                    CancellationToken::new(),
                    None,
                    std::sync::Arc::new(std::sync::RwLock::new(crate::SessionState::default())),
                    None,
                )
                .await;

            assert!(!result.is_error);
            assert!(result.is_transfer());
        }

        // T022: Empty allowed_targets set rejects all transfers
        #[tokio::test]
        async fn transfer_tool_empty_allowed_targets_rejects_all() {
            let registry = std::sync::Arc::new(AgentRegistry::new());
            registry.register("billing", dummy_agent());

            // Empty allowed targets = nothing allowed
            let tool = TransferToAgentTool::with_allowed_targets(
                std::sync::Arc::clone(&registry),
                std::iter::empty::<String>(),
            );
            let params = serde_json::json!({
                "agent_name": "billing",
                "reason": "test"
            });

            let result = tool
                .execute(
                    "tc-1",
                    params,
                    CancellationToken::new(),
                    None,
                    std::sync::Arc::new(std::sync::RwLock::new(crate::SessionState::default())),
                    None,
                )
                .await;

            assert!(result.is_error);
            let text = &result.content[0];
            match text {
                crate::types::ContentBlock::Text { text } => {
                    assert!(
                        text.contains("not allowed"),
                        "expected 'not allowed', got: {text}"
                    );
                }
                _ => panic!("expected text content block"),
            }
        }

        // Additional: cancellation token respected
        #[tokio::test]
        async fn transfer_tool_respects_cancellation() {
            let registry = std::sync::Arc::new(AgentRegistry::new());
            registry.register("billing", dummy_agent());

            let tool = TransferToAgentTool::new(registry);
            let params = serde_json::json!({
                "agent_name": "billing",
                "reason": "test"
            });

            let token = CancellationToken::new();
            token.cancel();

            let result = tool
                .execute(
                    "tc-1",
                    params,
                    token,
                    None,
                    std::sync::Arc::new(std::sync::RwLock::new(crate::SessionState::default())),
                    None,
                )
                .await;

            assert!(result.is_error);
            let text = &result.content[0];
            match text {
                crate::types::ContentBlock::Text { text } => {
                    assert_eq!(text, "cancelled");
                }
                _ => panic!("expected text content block"),
            }
        }
    }
}
