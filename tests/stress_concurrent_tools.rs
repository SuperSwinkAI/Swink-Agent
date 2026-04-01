//! Stress test: 12 concurrent tool calls with random delays, verifying no
//! results are duplicated or lost.

mod common;

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use swink_agent::{
    Agent, AgentEvent, AgentMessage, AgentOptions, AgentTool, AgentToolResult, ContentBlock,
    LlmMessage,
};

use common::{
    MockStreamFn, MockTool, default_convert, default_model, text_events, tool_call_events_multi,
    user_msg,
};

const TOOL_COUNT: usize = 12;

/// Tool names used for the 12 mock tools.
const TOOL_NAMES: [&str; TOOL_COUNT] = [
    "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india", "juliet",
    "kilo", "lima",
];

/// Deterministic per-tool delays in milliseconds (1-50ms range).
const DELAYS_MS: [u64; TOOL_COUNT] = [23, 7, 42, 15, 1, 38, 11, 50, 3, 29, 19, 46];

fn build_tool_calls() -> Vec<(String, String, String)> {
    TOOL_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| (format!("tc_{i}"), name.to_string(), "{}".to_string()))
        .collect()
}

fn build_tools() -> Vec<Arc<dyn AgentTool>> {
    TOOL_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| {
            Arc::new(
                MockTool::new(name)
                    .with_delay(Duration::from_millis(DELAYS_MS[i]))
                    .with_result(AgentToolResult::text(format!("result_from_{name}"))),
            ) as Arc<dyn AgentTool>
        })
        .collect()
}

fn extract_tool_result_texts(messages: &[AgentMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|msg| match msg {
            AgentMessage::Llm(LlmMessage::ToolResult(tr)) => {
                tr.content.iter().find_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn twelve_concurrent_tool_calls_no_duplicates_no_lost() {
    let calls = build_tool_calls();
    let call_refs: Vec<(&str, &str, &str)> = calls
        .iter()
        .map(|(id, name, args)| (id.as_str(), name.as_str(), args.as_str()))
        .collect();

    // First response: 12 tool calls. Second response: text-only "done".
    let responses = vec![tool_call_events_multi(&call_refs), text_events("done")];

    let stream_fn = Arc::new(MockStreamFn::new(responses));
    let tools = build_tools();

    let opts = AgentOptions::new(
        "You are a tool-using assistant.",
        default_model(),
        stream_fn,
        default_convert,
    )
    .with_tools(tools);

    let mut agent = Agent::new(opts);

    // Track execution start/end events and capture tool names from start events.
    let exec_start_count = Arc::new(AtomicUsize::new(0));
    let exec_end_count = Arc::new(AtomicUsize::new(0));
    let started_names: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let start_clone = Arc::clone(&exec_start_count);
    let end_clone = Arc::clone(&exec_end_count);
    let names_clone = Arc::clone(&started_names);

    agent.subscribe(move |event: &AgentEvent| match event {
        AgentEvent::ToolExecutionStart { name, .. } => {
            start_clone.fetch_add(1, Ordering::SeqCst);
            names_clone.lock().unwrap().push(name.clone());
        }
        AgentEvent::ToolExecutionEnd { .. } => {
            end_clone.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    });

    // Run with a 30-second timeout.
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        agent.prompt_async(vec![user_msg("use all 12 tools")]),
    )
    .await;

    assert!(result.is_ok(), "agent timed out after 30s (deadlock?)");
    let agent_result = result.unwrap();
    assert!(
        agent_result.is_ok(),
        "agent returned error: {:?}",
        agent_result.err()
    );
    let agent_result = agent_result.unwrap();

    // ── Verify all 12 ToolExecutionStart events received ──
    let starts = exec_start_count.load(Ordering::SeqCst);
    assert_eq!(
        starts, TOOL_COUNT,
        "expected {TOOL_COUNT} ToolExecutionStart events, got {starts}"
    );

    // ── Verify all 12 ToolExecutionEnd events received ──
    let ends = exec_end_count.load(Ordering::SeqCst);
    assert_eq!(
        ends, TOOL_COUNT,
        "expected {TOOL_COUNT} ToolExecutionEnd events, got {ends}"
    );

    // ── Verify no duplicate start names ──
    let names = started_names.lock().unwrap().clone();
    let unique_names: HashSet<&String> = names.iter().collect();
    assert_eq!(
        unique_names.len(),
        TOOL_COUNT,
        "expected {TOOL_COUNT} unique tool names in start events, got {} (duplicates detected)",
        unique_names.len()
    );

    // ── Verify all 12 tool results present in final messages ──
    let tool_result_texts = extract_tool_result_texts(&agent_result.messages);

    // Each tool should have exactly one result.
    assert_eq!(
        tool_result_texts.len(),
        TOOL_COUNT,
        "expected {TOOL_COUNT} tool result messages, got {}",
        tool_result_texts.len()
    );

    // Verify each tool name appears in exactly one result.
    for name in &TOOL_NAMES {
        let expected = format!("result_from_{name}");
        let count = tool_result_texts.iter().filter(|t| **t == expected).count();
        assert_eq!(
            count, 1,
            "tool '{name}': expected exactly 1 result with text '{expected}', found {count}"
        );
    }

    // No duplicates: unique results should equal total results.
    let unique_results: HashSet<&String> = tool_result_texts.iter().collect();
    assert_eq!(
        unique_results.len(),
        tool_result_texts.len(),
        "duplicate tool results detected"
    );
}
