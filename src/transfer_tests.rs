//! Tests for `transfer`.
#![cfg(test)]

use super::*;

// T007: TransferSignal constructors, accessors, and serde round-trip

#[test]
fn transfer_signal_new_sets_target_and_reason() {
    let signal = TransferSignal::new("billing", "billing issue");
    assert_eq!(signal.target_agent(), "billing");
    assert_eq!(signal.reason(), "billing issue");
    assert_eq!(signal.context_summary(), None);
    assert!(signal.conversation_history().is_empty());
    assert!(signal.transfer_chain().is_none());
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
    let signal = TransferSignal::new("tech", "tech issue").with_conversation_history(vec![msg]);
    assert_eq!(signal.conversation_history().len(), 1);
}

#[test]
fn transfer_signal_serde_roundtrip() {
    let mut chain = TransferChain::new(3);
    chain.push("support").unwrap();
    chain.push("billing").unwrap();
    let signal = TransferSignal::new("billing", "billing issue")
        .with_context_summary("User disputes charge")
        .with_transfer_chain(chain);
    let json = serde_json::to_string(&signal).unwrap();
    let parsed: TransferSignal = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.target_agent(), "billing");
    assert_eq!(parsed.reason(), "billing issue");
    assert_eq!(parsed.context_summary(), Some("User disputes charge"));
    assert!(parsed.conversation_history().is_empty());
    let chain = parsed.transfer_chain().expect("expected transfer chain");
    assert_eq!(chain.chain(), &["support", "billing"]);
}

#[test]
fn transfer_signal_deserialize_without_optional_fields() {
    let json = r#"{"target_agent":"billing","reason":"billing issue"}"#;
    let parsed: TransferSignal = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.target_agent(), "billing");
    assert_eq!(parsed.reason(), "billing issue");
    assert_eq!(parsed.context_summary(), None);
    assert!(parsed.conversation_history().is_empty());
    assert!(parsed.transfer_chain().is_none());
}

#[test]
fn transfer_signal_serde_skips_none_context_summary() {
    let signal = TransferSignal::new("billing", "billing issue");
    let json = serde_json::to_value(&signal).unwrap();
    assert!(!json.as_object().unwrap().contains_key("context_summary"));
    assert!(!json.as_object().unwrap().contains_key("transfer_chain"));
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
        let tool = TransferToAgentTool::with_allowed_targets(registry, vec!["billing"]);
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

        let tool = TransferToAgentTool::with_allowed_targets(registry, vec!["billing"]);
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
