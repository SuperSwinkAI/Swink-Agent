//! Transfer types for agent-to-agent handoff signaling.
//!
//! This module provides the [`TransferSignal`] type that carries handoff
//! context between agents, and will eventually include [`TransferToAgentTool`],
//! [`TransferChain`], and [`TransferError`] for the full transfer system.

use serde::{Deserialize, Serialize};

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

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<TransferSignal>();
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
}
