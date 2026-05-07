#![cfg(feature = "testkit")]
//! Stress test: 50 concurrent tool calls proving parallel execution.

mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use swink_agent::{
    Agent, AgentEvent, AgentOptions, AgentTool, AgentToolResult, ResolvedCredential, SessionState,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use common::{
    MockStreamFn, default_convert, default_model, text_events, tool_call_events_multi, user_msg,
};

const TOOL_COUNT: usize = 50;

struct StressGateTool {
    name: String,
    schema: serde_json::Value,
    started_count: Arc<AtomicUsize>,
    all_started: Arc<Notify>,
}

impl StressGateTool {
    fn new(name: String, started_count: Arc<AtomicUsize>, all_started: Arc<Notify>) -> Self {
        Self {
            name,
            schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": true
            }),
            started_count,
            all_started,
        }
    }
}

impl AgentTool for StressGateTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &'static str {
        "stress gate tool"
    }

    fn parameters_schema(&self) -> &serde_json::Value {
        &self.schema
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<SessionState>>,
        _credential: Option<ResolvedCredential>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        let started_count = Arc::clone(&self.started_count);
        let all_started = Arc::clone(&self.all_started);
        Box::pin(async move {
            let started = started_count.fetch_add(1, Ordering::SeqCst) + 1;
            if started == TOOL_COUNT {
                all_started.notify_waiters();
            }

            while started_count.load(Ordering::SeqCst) < TOOL_COUNT {
                all_started.notified().await;
            }

            AgentToolResult::text("ok")
        })
    }
}

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
    let responses = vec![tool_call_events_multi(&call_refs), text_events("done")];

    let stream_fn = Arc::new(MockStreamFn::new(responses));

    let started_count = Arc::new(AtomicUsize::new(0));
    let all_started = Arc::new(Notify::new());

    // Register 50 tools that cannot finish until every tool has started.
    let tools: Vec<Arc<dyn swink_agent::AgentTool>> = (0..TOOL_COUNT)
        .map(|i| {
            Arc::new(StressGateTool::new(
                format!("tool_{i}"),
                Arc::clone(&started_count),
                Arc::clone(&all_started),
            )) as Arc<dyn swink_agent::AgentTool>
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

    agent.subscribe(move |event: &AgentEvent| match event {
        AgentEvent::ToolExecutionStart { .. } => {
            start_clone.fetch_add(1, Ordering::SeqCst);
        }
        AgentEvent::ToolExecutionEnd { .. } => {
            end_clone.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    });

    let result = tokio::time::timeout(
        Duration::from_secs(15),
        agent.prompt_async(vec![user_msg("use all tools")]),
    )
    .await;

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
}
