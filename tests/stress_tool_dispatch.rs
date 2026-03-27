//! Stress test: 50 concurrent tool calls proving parallel execution.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use swink_agent::{Agent, AgentEvent, AgentOptions};

use common::{
    default_convert, default_model, text_events, tool_call_events_multi, user_msg, MockTool,
    MockStreamFn,
};

const TOOL_COUNT: usize = 50;
const TOOL_DELAY_MS: u64 = 10;

#[tokio::test]
async fn fifty_concurrent_tool_calls() {
    // Build 50 tool call entries.
    let calls: Vec<(String, String, String)> = (0..TOOL_COUNT)
        .map(|i| (format!("tc_{i}"), format!("tool_{i}"), "{}".to_string()))
        .collect();
    let call_refs: Vec<(&str, &str, &str)> = calls
        .iter()
        .map(|(id, name, args)| (id.as_str(), name.as_str(), args.as_str()))
        .collect();

    // First response: 50 tool calls. Second response: text-only "done".
    let responses = vec![
        tool_call_events_multi(&call_refs),
        text_events("done"),
    ];

    let stream_fn = Arc::new(MockStreamFn::new(responses));

    // Register 50 tools, each with a 10ms async delay.
    let tools: Vec<Arc<dyn swink_agent::AgentTool>> = (0..TOOL_COUNT)
        .map(|i| {
            Arc::new(
                MockTool::new(&format!("tool_{i}")).with_delay(Duration::from_millis(TOOL_DELAY_MS)),
            ) as Arc<dyn swink_agent::AgentTool>
        })
        .collect();

    let opts = AgentOptions::new(
        "You are a tool-using assistant.",
        default_model(),
        stream_fn,
        default_convert,
    )
    .with_tools(tools);

    let mut agent = Agent::new(opts);

    // Track execution start/end events.
    let exec_start_count = Arc::new(AtomicUsize::new(0));
    let exec_end_count = Arc::new(AtomicUsize::new(0));
    let start_clone = Arc::clone(&exec_start_count);
    let end_clone = Arc::clone(&exec_end_count);

    agent.subscribe(move |event: &AgentEvent| {
        match event {
            AgentEvent::ToolExecutionStart { .. } => {
                start_clone.fetch_add(1, Ordering::SeqCst);
            }
            AgentEvent::ToolExecutionEnd { .. } => {
                end_clone.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
    });

    let start = Instant::now();

    let result = tokio::time::timeout(
        Duration::from_secs(15),
        agent.prompt_async(vec![user_msg("use all tools")]),
    )
    .await;

    let elapsed = start.elapsed();

    assert!(result.is_ok(), "agent timed out after 15s");
    let agent_result = result.unwrap();
    assert!(
        agent_result.is_ok(),
        "agent returned error: {:?}",
        agent_result.err()
    );

    let starts = exec_start_count.load(Ordering::SeqCst);
    let ends = exec_end_count.load(Ordering::SeqCst);

    assert_eq!(
        starts, TOOL_COUNT,
        "expected {TOOL_COUNT} ToolExecutionStart events, got {starts}"
    );
    assert_eq!(
        ends, TOOL_COUNT,
        "expected {TOOL_COUNT} ToolExecutionEnd events, got {ends}"
    );

    // If tools ran sequentially, it would take at least 50 × 10ms = 500ms.
    // With concurrent execution, it should be much less.
    assert!(
        elapsed < Duration::from_millis(500),
        "tools took {elapsed:?}, which suggests sequential execution (expected < 500ms)"
    );
}
