//! Stream and channel resilience tests.
//!
//! Validates that malformed JSON in tool calls produces correct errors,
//! agents handle high event volumes without dropping events, and
//! dropped subscribers do not cause panics.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{
    EventCollector, MockTool, default_convert, default_model, text_only_events, user_msg,
};

use swink_agent::testing::ScriptedStreamFn;
use swink_agent::{
    Agent, AgentOptions, AssistantMessageEvent, DefaultRetryStrategy, StreamFn, accumulate_message,
};

// ═══════════════════════════════════════════════════════════════════════════
// 1 — Malformed JSON tool call produces accumulation error
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn malformed_json_tool_call_returns_error() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "call_1".into(),
            name: "test_tool".into(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "{invalid}".into(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
    ];

    let result = accumulate_message(events, "test", "test-model");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("failed to parse arguments JSON"),
        "expected 'failed to parse arguments JSON', got: {err}"
    );
}

#[test]
fn unterminated_string_tool_call_returns_error() {
    let events = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "call_2".into(),
            name: "test_tool".into(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "\"unterminated".into(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
    ];

    let result = accumulate_message(events, "test", "test-model");
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("failed to parse arguments JSON"),
        "expected 'failed to parse arguments JSON', got: {err}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 2 — Event channel handles high event volume without drops
// ═══════════════════════════════════════════════════════════════════════════

fn make_agent(stream_fn: Arc<dyn StreamFn>) -> Agent {
    Agent::new(
        AgentOptions::new("test prompt", default_model(), stream_fn, default_convert)
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    )
}

fn make_agent_with_tools(
    stream_fn: Arc<dyn StreamFn>,
    tools: Vec<Arc<dyn swink_agent::AgentTool>>,
) -> Agent {
    Agent::new(
        AgentOptions::new("test prompt", default_model(), stream_fn, default_convert)
            .with_tools(tools)
            .with_retry_strategy(Box::new(
                DefaultRetryStrategy::default()
                    .with_jitter(false)
                    .with_base_delay(Duration::from_millis(1)),
            )),
    )
}

#[tokio::test]
async fn many_tool_calls_no_dropped_events() {
    // Build a response with many tool calls to generate lots of events.
    let num_calls = 20;
    let mut calls: Vec<(&str, &str, &str)> = Vec::new();
    let ids: Vec<String> = (0..num_calls).map(|i| format!("tc_{i}")).collect();
    for id in &ids {
        calls.push((id.as_str(), "mock_tool", "{}"));
    }

    let tool_response = swink_agent::testing::tool_call_events_multi(&calls);
    // After tool execution, the agent will call the LLM again — give it a text reply.
    let final_response = text_only_events("done");

    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![tool_response, final_response]));
    let tool: Arc<dyn swink_agent::AgentTool> = Arc::new(MockTool::new("mock_tool"));
    let mut agent = make_agent_with_tools(stream_fn, vec![tool]);

    let collector = EventCollector::new();
    let _sub = agent.subscribe(collector.subscriber());

    let _result = agent.prompt_async(vec![user_msg("run tools")]).await;

    let events = collector.events();

    // We expect lifecycle events: AgentStart, TurnStart, MessageStart, MessageUpdate(s),
    // MessageEnd, ToolExecutionStart/End per tool, then another turn with text.
    // The key assertion: we got a substantial number of events and no panics.
    assert!(
        events.len() > num_calls,
        "expected more than {num_calls} events, got {}",
        events.len()
    );

    // Verify we got tool execution events for each tool call.
    let tool_exec_starts = events.iter().filter(|e| *e == "ToolExecutionStart").count();
    assert_eq!(
        tool_exec_starts, num_calls,
        "expected {num_calls} ToolExecutionStart events, got {tool_exec_starts}"
    );

    let tool_exec_ends = events.iter().filter(|e| *e == "ToolExecutionEnd").count();
    assert_eq!(
        tool_exec_ends, num_calls,
        "expected {num_calls} ToolExecutionEnd events, got {tool_exec_ends}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 3 — Dropping subscriber mid-stream does not cause panic
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn dropped_subscriber_does_not_panic() {
    let stream_fn = Arc::new(ScriptedStreamFn::new(vec![text_only_events("hello world")]));
    let mut agent = make_agent(stream_fn);

    // Subscribe then immediately drop the subscription handle.
    let sub = agent.subscribe(|_event: &swink_agent::AgentEvent| {
        // Intentionally empty — we just want a subscriber that exists briefly.
    });
    let _ = sub;

    // Run a prompt — the agent should complete without panic even though
    // the subscriber was dropped.
    let result = agent.prompt_async(vec![user_msg("hi")]).await;
    assert!(
        result.is_ok(),
        "agent should complete after subscriber drop: {:?}",
        result.err()
    );
}
