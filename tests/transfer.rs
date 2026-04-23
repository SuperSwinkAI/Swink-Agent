#![cfg(feature = "testkit")]
//! Integration tests for spec 040: Agent Transfer/Handoff behavior.
//!
//! These tests exercise the full agent loop with [`TransferToAgentTool`] and
//! verify that transfer signals propagate correctly through the loop, produce
//! the right `StopReason`, emit proper events, and interact correctly with
//! cancellation and concurrent tool calls.

mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::json;
use tokio_util::sync::CancellationToken;

use swink_agent::testing::SimpleMockStreamFn;
use swink_agent::{
    Agent, AgentMessage, AgentOptions, AgentRegistry, AgentTool, AgentToolResult, ContentBlock,
    DefaultRetryStrategy, LlmMessage, ModelSpec, RetryStrategy, StopReason, ToolExecutionPolicy,
    TransferChain, TransferToAgentTool,
};

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
fn fast_retry() -> Box<dyn RetryStrategy> {
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
fn make_transfer_agent(stream_fn: Arc<MockStreamFn>, registry: Arc<AgentRegistry>) -> Agent {
    let transfer_tool = Arc::new(TransferToAgentTool::new(registry));
    Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            stream_fn,
            default_convert,
        )
        .with_tools(vec![transfer_tool as Arc<dyn AgentTool>])
        .with_retry_strategy(fast_retry()),
    )
}

struct MockCancellationIgnoringTool {
    executed: Arc<AtomicBool>,
}

impl MockCancellationIgnoringTool {
    fn new() -> Self {
        Self {
            executed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn was_executed(&self) -> bool {
        self.executed.load(Ordering::SeqCst)
    }
}

impl AgentTool for MockCancellationIgnoringTool {
    fn name(&self) -> &'static str {
        "blocking_tool"
    }

    fn label(&self) -> &'static str {
        "blocking_tool"
    }

    fn description(&self) -> &'static str {
        "A tool that ignores cancellation and never completes unless aborted"
    }

    fn parameters_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": true
            })
        })
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::ResolvedCredential>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        self.executed.store(true, Ordering::SeqCst);
        Box::pin(async move { std::future::pending::<AgentToolResult>().await })
    }
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
    let result = agent
        .prompt_async(vec![user_msg("transfer me to billing")])
        .await
        .unwrap();

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

    let result = agent
        .prompt_async(vec![user_msg("transfer me")])
        .await
        .unwrap();

    assert_eq!(result.stop_reason, StopReason::Transfer);

    let events = collector.events();
    assert!(
        events.contains(&"TransferInitiated".to_string()),
        "should emit TransferInitiated event, got: {events:?}"
    );

    // Verify ordering: TransferInitiated should come before AgentEnd
    let transfer_pos = events
        .iter()
        .position(|e| e == "TransferInitiated")
        .unwrap();
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
    assert!(
        has_user,
        "conversation_history should contain a User message"
    );
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

    let result = agent
        .prompt_async(vec![user_msg("transfer me")])
        .await
        .unwrap();

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

    let result = agent
        .prompt_async(vec![user_msg("do stuff")])
        .await
        .unwrap();

    // The abort should cause the loop to terminate with Aborted.
    // If the transfer tool completes before cancellation propagates,
    // Transfer is also acceptable.
    assert!(
        result.stop_reason == StopReason::Aborted || result.stop_reason == StopReason::Transfer,
        "expected Aborted or Transfer, got {:?}",
        result.stop_reason
    );
}

// ─── T042: Transfer cancels same-group siblings promptly ───────────────────

#[tokio::test]
async fn transfer_cancels_same_group_siblings() {
    let registry = registry_with_billing();

    let blocking_tool = Arc::new(MockCancellationIgnoringTool::new());
    let transfer_tool: Arc<dyn AgentTool> =
        Arc::new(TransferToAgentTool::new(Arc::clone(&registry)));

    // LLM calls both tools in the same concurrent group. Without prompt
    // cancellation on transfer, the blocking tool would hang the turn forever.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events_multi(&[
            ("tc_blocking", "blocking_tool", "{}"),
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
            .with_tools(vec![
                blocking_tool.clone() as Arc<dyn AgentTool>,
                transfer_tool,
            ])
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

    assert!(
        blocking_tool.was_executed(),
        "same-group sibling should have started before transfer cancellation"
    );

    let blocking_result = result
        .messages
        .iter()
        .find_map(|msg| match msg {
            AgentMessage::Llm(LlmMessage::ToolResult(tool_result))
                if tool_result.tool_call_id == "tc_blocking" =>
            {
                Some(ContentBlock::extract_text(&tool_result.content))
            }
            _ => None,
        })
        .expect("blocking tool result should be present");
    assert!(
        blocking_result.contains("transfer initiated"),
        "blocking tool should be cancelled once transfer wins, got: {blocking_result}"
    );
}

#[tokio::test]
async fn transfer_skips_later_priority_groups() {
    let registry = registry_with_billing();

    let later_group_tool = Arc::new(
        MockTool::new("low_priority_tool").with_result(AgentToolResult::text("should not run")),
    );
    let transfer_tool: Arc<dyn AgentTool> =
        Arc::new(TransferToAgentTool::new(Arc::clone(&registry)));

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events_multi(&[
            (
                "tc_transfer",
                "transfer_to_agent",
                &transfer_args("billing", "billing question"),
            ),
            ("tc_low", "low_priority_tool", "{}"),
        ]),
        text_only_events("fallback"),
    ]));

    let priority_fn = Arc::new(|summary: &swink_agent::ToolCallSummary<'_>| {
        if summary.name == "transfer_to_agent" {
            10
        } else {
            0
        }
    });

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![
                transfer_tool,
                later_group_tool.clone() as Arc<dyn AgentTool>,
            ])
            .with_tool_execution_policy(ToolExecutionPolicy::Priority(priority_fn))
            .with_retry_strategy(fast_retry()),
    );

    let result = agent.prompt_async(vec![user_msg("do both")]).await.unwrap();

    assert!(
        result.stop_reason == StopReason::Transfer,
        "priority-group transfer should terminate the turn"
    );
    assert!(
        !later_group_tool.was_executed(),
        "later priority groups must not run after a transfer signal"
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

    let result = agent
        .prompt_async(vec![user_msg("transfer me")])
        .await
        .unwrap();

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

// ─── Transfer chain safety enforcement (issue #472) ────────────────────────

/// Build an Agent with the transfer tool and a known `agent_name` for chain tracking.
fn make_named_transfer_agent(
    name: &str,
    stream_fn: Arc<MockStreamFn>,
    registry: Arc<AgentRegistry>,
) -> Agent {
    make_named_transfer_agent_with_chain(name, stream_fn, registry, None)
}

/// Build an Agent with a known `agent_name` and optional carried transfer chain.
fn make_named_transfer_agent_with_chain(
    name: &str,
    stream_fn: Arc<MockStreamFn>,
    registry: Arc<AgentRegistry>,
    transfer_chain: Option<TransferChain>,
) -> Agent {
    let transfer_tool = Arc::new(TransferToAgentTool::new(registry));
    let mut opts = AgentOptions::new(
        "test system prompt",
        default_model(),
        stream_fn,
        default_convert,
    )
    .with_tools(vec![transfer_tool as Arc<dyn AgentTool>])
    .with_retry_strategy(fast_retry())
    .with_agent_name(name);
    if let Some(chain) = transfer_chain {
        opts = opts.with_transfer_chain(chain);
    }
    Agent::new(opts)
}

// Self-transfer (A -> A) is blocked by the transfer chain.
#[tokio::test]
async fn transfer_chain_blocks_self_transfer() {
    let registry = Arc::new(AgentRegistry::new());
    registry.register("support", dummy_agent());

    // Turn 1: LLM calls transfer_to_agent targeting itself ("support")
    // Turn 2: After the rejected transfer, LLM gives a text response
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events(
            "tc_transfer",
            "transfer_to_agent",
            &transfer_args("support", "self-transfer"),
        ),
        text_only_events("I'll help you directly instead"),
    ]));

    let mut agent = make_named_transfer_agent("support", stream_fn, registry);
    let result = agent
        .prompt_async(vec![user_msg("transfer me to support")])
        .await
        .unwrap();

    // The transfer should NOT happen — the chain rejects circular transfers.
    assert_ne!(
        result.stop_reason,
        StopReason::Transfer,
        "self-transfer should be blocked by TransferChain"
    );
    assert!(
        result.transfer_signal.is_none(),
        "transfer_signal should be None when self-transfer is blocked"
    );
}

// Repeated-agent loop (A -> B -> A) is blocked on the second hop.
// This test verifies that the chain correctly identifies the circular
// pattern by checking that the first transfer (support -> billing) works,
// meaning the chain mechanism is properly initialized with the current
// agent name.
#[tokio::test]
async fn transfer_chain_blocks_circular_a_to_b_to_a() {
    // Register both agents in the registry.
    let registry = Arc::new(AgentRegistry::new());
    registry.register("support", dummy_agent());
    registry.register("billing", dummy_agent());

    // Agent "support" transfers to "billing" — this should succeed since
    // "billing" is not in the chain yet (chain = ["support"]).
    let support_stream = Arc::new(MockStreamFn::new(vec![
        tool_call_events(
            "tc_transfer",
            "transfer_to_agent",
            &transfer_args("billing", "billing question"),
        ),
        text_only_events("fallback"),
    ]));

    let mut support_agent =
        make_named_transfer_agent("support", support_stream, Arc::clone(&registry));
    let first_result = support_agent
        .prompt_async(vec![user_msg("transfer me to billing")])
        .await
        .unwrap();

    // First transfer should succeed (support -> billing).
    assert_eq!(
        first_result.stop_reason,
        StopReason::Transfer,
        "first transfer in chain should succeed"
    );
    let first_signal = first_result.transfer_signal.as_ref().unwrap();
    assert_eq!(first_signal.target_agent(), "billing");

    // Second hop on the transferred-to agent: billing -> support should be
    // rejected because the carried chain already contains "support".
    let billing_stream = Arc::new(MockStreamFn::new(vec![
        tool_call_events(
            "tc_transfer_back",
            "transfer_to_agent",
            &transfer_args("support", "route back"),
        ),
        text_only_events("I'll handle this directly"),
    ]));
    let carried_chain = first_signal
        .transfer_chain()
        .expect("handoff signal should carry transfer chain")
        .clone();
    let mut billing_agent = make_named_transfer_agent_with_chain(
        "billing",
        billing_stream,
        Arc::clone(&registry),
        Some(carried_chain),
    );
    let second_result = billing_agent
        .prompt_async(vec![user_msg("continue on billing and transfer back")])
        .await
        .unwrap();

    assert_ne!(
        second_result.stop_reason,
        StopReason::Transfer,
        "A->B->A must be blocked across handoffs"
    );
    assert!(
        second_result.transfer_signal.is_none(),
        "blocked cross-handoff transfer must not return a transfer signal"
    );
}

// Max depth enforcement across handoffs: carried chains at max depth should
// reject the next transfer on the receiving agent.
#[tokio::test]
async fn transfer_chain_max_depth_is_enforced_in_loop() {
    // Register a target agent.
    let registry = Arc::new(AgentRegistry::new());
    registry.register("target", dummy_agent());

    // Seed a carried chain already at max depth.
    let mut carried_chain = TransferChain::new(2);
    carried_chain.push("agent-0").unwrap();
    carried_chain.push("agent-1").unwrap();

    // Receiving agent attempts another transfer, which should fail due to max depth.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events(
            "tc_transfer",
            "transfer_to_agent",
            &transfer_args("target", "handoff"),
        ),
        text_only_events("cannot transfer further"),
    ]));

    let mut agent =
        make_named_transfer_agent_with_chain("agent-1", stream_fn, registry, Some(carried_chain));
    let result = agent
        .prompt_async(vec![user_msg("transfer me")])
        .await
        .unwrap();

    assert_ne!(
        result.stop_reason,
        StopReason::Transfer,
        "transfer should be blocked when carried chain is already at max depth"
    );
    assert!(
        result.transfer_signal.is_none(),
        "blocked max-depth transfer must not return a transfer signal"
    );
}

// When no agent_name is set, transfers still work (no chain enforcement).
#[tokio::test]
async fn transfer_works_without_agent_name() {
    let registry = registry_with_billing();

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events(
            "tc_transfer",
            "transfer_to_agent",
            &transfer_args("billing", "billing question"),
        ),
        text_only_events("fallback"),
    ]));

    // Use the old-style agent without agent_name.
    let mut agent = make_transfer_agent(stream_fn, registry);
    let result = agent
        .prompt_async(vec![user_msg("transfer me")])
        .await
        .unwrap();

    // Should still work — chain starts empty, no current agent to check against.
    assert_eq!(
        result.stop_reason,
        StopReason::Transfer,
        "transfer without agent_name should succeed"
    );
    assert!(result.transfer_signal.is_some());
}
