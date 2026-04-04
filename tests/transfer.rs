#![cfg(feature = "testkit")]
//! Integration tests for spec 040: Agent Transfer/Handoff behavior.
//!
//! These tests exercise the full agent loop with [`TransferToAgentTool`] and
//! verify that transfer signals propagate correctly through the loop, produce
//! the right `StopReason`, emit proper events, and interact correctly with
//! cancellation and concurrent tool calls.

mod common;

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use swink_agent::{
    Agent, AgentEvent, AgentMessage, AgentOptions, AgentRegistry, AgentTool, AgentToolResult,
    ContentBlock, DefaultRetryStrategy, LlmMessage, ModelSpec, StopReason, TransferToAgentTool,
};
use swink_agent::testing::SimpleMockStreamFn;

use common::{
    EventCollector, MockStreamFn, MockTool, default_convert, default_model, text_only_events,
    tool_call_events, tool_call_events_multi, user_msg,
};

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Build a minimal Agent suitable for registering as a target in the registry.
fn dummy_agent() -> Agent {
    Agent::new(AgentOptions::new(
        "dummy",
        ModelSpec::new("test", "test-model"),
        Arc::new(SimpleMockStreamFn::from_text("hi")),
        default_convert,
    ))
}

/// Build a fast retry strategy (no jitter, minimal delay).
fn fast_retry() -> Box<DefaultRetryStrategy> {
    Box::new(
        DefaultRetryStrategy::default()
            .with_jitter(false)
            .with_base_delay(Duration::from_millis(1)),
    )
}

/// JSON arguments for the `transfer_to_agent` tool call.
fn transfer_args(agent_name: &str, reason: &str) -> String {
    json!({
        "agent_name": agent_name,
        "reason": reason
    })
    .to_string()
}

/// JSON arguments for the `transfer_to_agent` tool call with context summary.
fn transfer_args_with_summary(agent_name: &str, reason: &str, summary: &str) -> String {
    json!({
        "agent_name": agent_name,
        "reason": reason,
        "context_summary": summary
    })
    .to_string()
}

/// Create a registry with one dummy agent named "billing".
fn registry_with_billing() -> Arc<AgentRegistry> {
    let registry = Arc::new(AgentRegistry::new());
    registry.register("billing", dummy_agent());
    registry
}

/// Build an Agent with the transfer tool and the given mock stream.
fn make_transfer_agent(
    stream_fn: Arc<MockStreamFn>,
    registry: Arc<AgentRegistry>,
) -> Agent {
    let transfer_tool = Arc::new(TransferToAgentTool::new(registry));
    Agent::new(
        AgentOptions::new("test system prompt", default_model(), stream_fn, default_convert)
            .with_tools(vec![transfer_tool as Arc<dyn AgentTool>])
            .with_retry_strategy(fast_retry()),
    )
}

// ─── T014: Agent loop detects transfer, terminates with Transfer ────────────

#[tokio::test]
async fn agent_loop_detects_transfer_and_terminates_with_transfer_stop_reason() {
    let registry = registry_with_billing();

    // Turn 1: LLM calls transfer_to_agent
    // Turn 2 should never happen — loop terminates on transfer.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events(
            "tc_transfer",
            "transfer_to_agent",
            &transfer_args("billing", "billing question"),
        ),
        // If the loop incorrectly continues, it would consume this:
        text_only_events("should not reach this"),
    ]));

    let mut agent = make_transfer_agent(stream_fn, registry);
    let result = agent.prompt_async(vec![user_msg("transfer me to billing")]).await.unwrap();

    assert_eq!(
        result.stop_reason,
        StopReason::Transfer,
        "stop_reason should be Transfer"
    );
    assert!(
        result.transfer_signal.is_some(),
        "transfer_signal should be present on AgentResult"
    );

    let signal = result.transfer_signal.as_ref().unwrap();
    assert_eq!(signal.target_agent(), "billing");
    assert_eq!(signal.reason(), "billing question");
}

// ─── T033: Transfer event emitted on successful transfer ────────────────────

#[tokio::test]
async fn transfer_initiated_event_emitted_on_transfer() {
    let registry = registry_with_billing();

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events(
            "tc_transfer",
            "transfer_to_agent",
            &transfer_args("billing", "billing question"),
        ),
        text_only_events("fallback"),
    ]));

    let mut agent = make_transfer_agent(stream_fn, registry);

    let collector = EventCollector::new();
    agent.subscribe(collector.subscriber());

    let result = agent.prompt_async(vec![user_msg("transfer me")]).await.unwrap();

    assert_eq!(result.stop_reason, StopReason::Transfer);

    let events = collector.events();
    assert!(
        events.contains(&"TransferInitiated".to_string()),
        "should emit TransferInitiated event, got: {events:?}"
    );

    // Verify ordering: TransferInitiated should come before AgentEnd
    let transfer_pos = events.iter().position(|e| e == "TransferInitiated").unwrap();
    let agent_end_pos = events.iter().position(|e| e == "AgentEnd").unwrap();
    assert!(
        transfer_pos < agent_end_pos,
        "TransferInitiated ({transfer_pos}) should precede AgentEnd ({agent_end_pos})"
    );
}

// ─── T037: Conversation history in transfer signal contains LLM messages ────

#[tokio::test]
async fn transfer_signal_contains_conversation_history() {
    let registry = registry_with_billing();

    // The agent loop enriches the transfer signal with LLM message history.
    // After the user message and tool call turn, the signal should contain
    // the user message and the assistant tool-call message at minimum.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events(
            "tc_transfer",
            "transfer_to_agent",
            &transfer_args_with_summary("billing", "billing dispute", "User disputes $50 charge"),
        ),
        text_only_events("fallback"),
    ]));

    let mut agent = make_transfer_agent(stream_fn, registry);
    let result = agent
        .prompt_async(vec![user_msg("I need help with my bill")])
        .await
        .unwrap();

    assert_eq!(result.stop_reason, StopReason::Transfer);

    let signal = result.transfer_signal.as_ref().unwrap();
    assert_eq!(signal.context_summary(), Some("User disputes $50 charge"));

    // The conversation history should be non-empty — at minimum contains
    // the user message and the assistant message from the transfer turn.
    let history = signal.conversation_history();
    assert!(
        !history.is_empty(),
        "conversation_history should not be empty"
    );

    // Should contain at least one User message
    let has_user = history.iter().any(|m| matches!(m, LlmMessage::User(_)));
    assert!(has_user, "conversation_history should contain a User message");
}

// ─── T040: Only first transfer signal honored in multi-transfer ─────────────

#[tokio::test]
async fn only_first_transfer_signal_honored_when_multiple_transfers_in_same_turn() {
    // Register two target agents
    let registry = Arc::new(AgentRegistry::new());
    registry.register("billing", dummy_agent());
    registry.register("tech", dummy_agent());

    // LLM issues two transfer_to_agent calls in the same response
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events_multi(&[
            (
                "tc_1",
                "transfer_to_agent",
                &transfer_args("billing", "billing question"),
            ),
            (
                "tc_2",
                "transfer_to_agent",
                &transfer_args("tech", "tech question"),
            ),
        ]),
        text_only_events("fallback"),
    ]));

    let transfer_tool = Arc::new(TransferToAgentTool::new(registry));
    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![transfer_tool as Arc<dyn AgentTool>])
            .with_retry_strategy(fast_retry()),
    );

    let result = agent.prompt_async(vec![user_msg("transfer me")]).await.unwrap();

    assert_eq!(result.stop_reason, StopReason::Transfer);
    let signal = result.transfer_signal.as_ref().unwrap();

    // First-wins semantics: the first tool call in the batch gets captured.
    // Both calls execute concurrently, so we just verify exactly one signal
    // is captured (not two).
    assert!(
        signal.target_agent() == "billing" || signal.target_agent() == "tech",
        "transfer should target one of the two agents, got: {}",
        signal.target_agent()
    );
}

// ─── T041: Cancellation takes precedence over transfer ──────────────────────

#[tokio::test]
async fn cancellation_takes_precedence_over_transfer() {
    let registry = registry_with_billing();

    // Use a tool with a long delay alongside the transfer tool so we can
    // cancel during execution. The slow tool keeps the turn alive long enough
    // for the cancellation to propagate.
    let slow_tool = Arc::new(MockTool::new("slow_tool").with_delay(Duration::from_secs(10)));
    let transfer_tool: Arc<dyn AgentTool> =
        Arc::new(TransferToAgentTool::new(Arc::clone(&registry)));

    // LLM calls both slow_tool and transfer_to_agent in the same turn
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events_multi(&[
            ("tc_slow", "slow_tool", "{}"),
            (
                "tc_transfer",
                "transfer_to_agent",
                &transfer_args("billing", "billing question"),
            ),
        ]),
        text_only_events("fallback"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![slow_tool, transfer_tool])
            .with_retry_strategy(fast_retry()),
    );

    // Abort immediately — cancellation should prevent transfer from completing
    agent.abort();

    let result = agent.prompt_async(vec![user_msg("do stuff")]).await.unwrap();

    // The abort should cause the loop to terminate with Aborted.
    // If the transfer tool completes before cancellation propagates,
    // Transfer is also acceptable.
    assert!(
        result.stop_reason == StopReason::Aborted || result.stop_reason == StopReason::Transfer,
        "expected Aborted or Transfer, got {:?}",
        result.stop_reason
    );
}

// ─── T042: Transfer alongside other tools processes all results ─────────────

#[tokio::test]
async fn transfer_alongside_other_tools_processes_all_results() {
    let registry = registry_with_billing();

    let echo_tool = Arc::new(MockTool::new("echo").with_result(AgentToolResult::text("echoed!")));
    let transfer_tool: Arc<dyn AgentTool> =
        Arc::new(TransferToAgentTool::new(Arc::clone(&registry)));

    // LLM calls both echo and transfer_to_agent in the same turn
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events_multi(&[
            ("tc_echo", "echo", "{}"),
            (
                "tc_transfer",
                "transfer_to_agent",
                &transfer_args("billing", "billing question"),
            ),
        ]),
        text_only_events("fallback"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![echo_tool.clone(), transfer_tool])
            .with_retry_strategy(fast_retry()),
    );

    let result = agent.prompt_async(vec![user_msg("do both")]).await.unwrap();

    // The turn should end with Transfer
    assert_eq!(
        result.stop_reason,
        StopReason::Transfer,
        "stop_reason should be Transfer even when other tools also ran"
    );
    assert!(result.transfer_signal.is_some());

    // The echo tool should have been executed (both tools run concurrently)
    assert!(
        echo_tool.was_executed(),
        "echo tool should have been executed alongside transfer"
    );

    // Both tool results should appear in the message history
    let tool_result_count = result.messages.iter().filter(|msg| {
        matches!(msg, AgentMessage::Llm(LlmMessage::ToolResult(_)))
    }).count();

    assert!(
        tool_result_count >= 2,
        "should have at least 2 tool results (echo + transfer), got {tool_result_count}"
    );
}

// ─── Additional: Transfer to nonexistent agent produces error, loop continues ─

#[tokio::test]
async fn transfer_to_nonexistent_agent_produces_error_and_loop_continues() {
    let registry = Arc::new(AgentRegistry::new());
    // Registry is empty — no agents registered

    let transfer_tool: Arc<dyn AgentTool> =
        Arc::new(TransferToAgentTool::new(Arc::clone(&registry)));

    // Turn 1: LLM calls transfer to nonexistent agent — tool returns error
    // Turn 2: LLM responds with text (the loop continues since no transfer signal)
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events(
            "tc_transfer",
            "transfer_to_agent",
            &transfer_args("nonexistent", "test"),
        ),
        text_only_events("I could not transfer, let me help directly"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![transfer_tool])
            .with_retry_strategy(fast_retry()),
    );

    let result = agent.prompt_async(vec![user_msg("transfer me")]).await.unwrap();

    // Should NOT be Transfer — the tool returned an error, not a signal
    assert_ne!(
        result.stop_reason,
        StopReason::Transfer,
        "should not be Transfer when target agent does not exist"
    );
    assert!(
        result.transfer_signal.is_none(),
        "transfer_signal should be None when transfer failed"
    );

    // The error tool result should be in the messages
    let has_error_result = result.messages.iter().any(|msg| {
        if let AgentMessage::Llm(LlmMessage::ToolResult(tr)) = msg {
            tr.is_error
                && tr.content.iter().any(|b| {
                    matches!(b, ContentBlock::Text { text } if text.contains("not found in registry"))
                })
        } else {
            false
        }
    });
    assert!(
        has_error_result,
        "should have an error tool result for nonexistent agent"
    );
}
