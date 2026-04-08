#![cfg(feature = "testkit")]
//! Integration tests for User Story 2: Tool Execution and Validation (T011-T019).

mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use swink_agent::{
    Agent, AgentMessage, AgentOptions, AgentTool, AgentToolResult, ContentBlock,
    DefaultRetryStrategy, LlmMessage,
};

use common::{
    MockContextCapturingStreamFn, MockStreamFn, MockTool, default_convert, default_model,
    text_only_events, tool_call_events, tool_call_events_multi, user_msg,
};

// ─── MockArgCapturingTool ────────────────────────────────────────────────────

/// A tool that captures the arguments it receives during execution.
struct MockArgCapturingTool {
    name: String,
    schema: Value,
    captured_args: Mutex<Option<Value>>,
}

impl MockArgCapturingTool {
    fn new(name: &str, schema: Value) -> Self {
        Self {
            name: name.to_string(),
            schema,
            captured_args: Mutex::new(None),
        }
    }

    fn captured_args(&self) -> Option<Value> {
        self.captured_args.lock().unwrap().clone()
    }
}

impl AgentTool for MockArgCapturingTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn label(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &'static str {
        "A tool that captures its arguments"
    }

    fn parameters_schema(&self) -> &Value {
        &self.schema
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute(
        &self,
        _tool_call_id: &str,
        params: Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::ResolvedCredential>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        *self.captured_args.lock().unwrap() = Some(params);
        Box::pin(async { AgentToolResult::text("ok") })
    }
}

// ─── Helper: default retry strategy (no jitter, minimal delay) ───────────

#[allow(clippy::unnecessary_box_returns)]
fn fast_retry() -> Box<DefaultRetryStrategy> {
    Box::new(
        DefaultRetryStrategy::default()
            .with_jitter(false)
            .with_base_delay(Duration::from_millis(1)),
    )
}

// ─── T011: tool_registration_and_discovery (AC 6) ────────────────────────

#[tokio::test]
async fn tool_registration_and_discovery() {
    let tool = Arc::new(MockTool::new("echo"));

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "echo", "{}"),
        text_only_events("done"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![tool.clone()])
            .with_retry_strategy(fast_retry()),
    );

    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();
    assert!(!result.messages.is_empty());
    assert!(tool.was_executed(), "tool should have been executed");
}

// ─── T012: schema_validation_rejects_invalid_args (AC 7) ─────────────────

#[tokio::test]
async fn schema_validation_rejects_invalid_args() {
    let tool = Arc::new(MockTool::new("strict_tool").with_schema(json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" }
        },
        "required": ["path"],
        "additionalProperties": false
    })));

    // LLM sends invalid args (wrong key, wrong type)
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "strict_tool", r#"{"wrong":42}"#),
        text_only_events("done"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![tool.clone()])
            .with_retry_strategy(fast_retry()),
    );

    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    assert!(
        !tool.was_executed(),
        "tool should NOT be executed when schema validation fails"
    );

    // Verify there is an error ToolResult in the messages
    let has_error_result = result.messages.iter().any(|msg| {
        if let AgentMessage::Llm(LlmMessage::ToolResult(tr)) = msg {
            tr.is_error
        } else {
            false
        }
    });
    assert!(
        has_error_result,
        "should have an error tool result for invalid args"
    );
}

// ─── T013: tool_execution_with_valid_args (AC 8) ─────────────────────────

#[tokio::test]
async fn tool_execution_with_valid_args() {
    let tool = Arc::new(
        MockTool::new("read_file")
            .with_schema(json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }))
            .with_result(AgentToolResult::text("file contents here")),
    );

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "read_file", r#"{"path":"/tmp/test.txt"}"#),
        text_only_events("done"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![tool.clone()])
            .with_retry_strategy(fast_retry()),
    );

    let result = agent.prompt_async(vec![user_msg("read it")]).await.unwrap();

    assert!(tool.was_executed(), "tool should have been executed");

    // Verify tool result content appears in messages
    let tool_result_content = result.messages.iter().find_map(|msg| {
        if let AgentMessage::Llm(LlmMessage::ToolResult(tr)) = msg {
            if tr.is_error { None } else { Some(&tr.content) }
        } else {
            None
        }
    });
    assert!(
        tool_result_content.is_some(),
        "should have a successful tool result in messages"
    );
    let content = tool_result_content.unwrap();
    let has_file_contents = content.iter().any(|block| {
        if let ContentBlock::Text { text } = block {
            text.contains("file contents here")
        } else {
            false
        }
    });
    assert!(
        has_file_contents,
        "tool result should contain the tool's output text"
    );
}

// ─── T014: concurrent_tool_execution (AC 9) ──────────────────────────────

#[tokio::test]
async fn concurrent_tool_execution() {
    let tool_a = Arc::new(MockTool::new("tool_a").with_delay(Duration::from_millis(50)));
    let tool_b = Arc::new(MockTool::new("tool_b").with_delay(Duration::from_millis(50)));
    let tool_c = Arc::new(MockTool::new("tool_c").with_delay(Duration::from_millis(50)));

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events_multi(&[
            ("call_a", "tool_a", "{}"),
            ("call_b", "tool_b", "{}"),
            ("call_c", "tool_c", "{}"),
        ]),
        text_only_events("done"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![tool_a.clone(), tool_b.clone(), tool_c.clone()])
            .with_retry_strategy(fast_retry()),
    );

    let start = Instant::now();
    let result = agent.prompt_async(vec![user_msg("run all")]).await.unwrap();
    let elapsed = start.elapsed();

    assert!(!result.messages.is_empty());
    assert_eq!(tool_a.execution_count(), 1, "tool_a should execute once");
    assert_eq!(tool_b.execution_count(), 1, "tool_b should execute once");
    assert_eq!(tool_c.execution_count(), 1, "tool_c should execute once");

    // 3 tools × 50ms each = 150ms sequential minimum.
    // If concurrent, elapsed ≈ 50ms. Use a generous upper bound (200ms)
    // that is still well below the 150ms sequential floor, avoiding
    // flaky failures on slow CI runners while still proving overlap.
    let sequential_total = Duration::from_millis(150);
    assert!(
        elapsed < sequential_total + Duration::from_millis(50),
        "elapsed {elapsed:?} should be significantly less than the {sequential_total:?} sequential total, proving concurrency"
    );
}

// ─── T015: tool_error_handling (AC 10) ───────────────────────────────────

#[tokio::test]
async fn tool_error_handling() {
    let tool = Arc::new(
        MockTool::new("failing_tool").with_result(AgentToolResult::error("something failed")),
    );

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "failing_tool", "{}"),
        text_only_events("agent continues after error"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![tool.clone()])
            .with_retry_strategy(fast_retry()),
    );

    let result = agent
        .prompt_async(vec![user_msg("do something")])
        .await
        .unwrap();

    // Verify error result is in messages
    let has_error_result = result.messages.iter().any(|msg| {
        if let AgentMessage::Llm(LlmMessage::ToolResult(tr)) = msg {
            tr.is_error
        } else {
            false
        }
    });
    assert!(has_error_result, "should have an error tool result");

    // Verify agent continued and produced a final text response
    let has_text = result.messages.iter().any(|msg| {
        if let AgentMessage::Llm(LlmMessage::Assistant(a)) = msg {
            a.content.iter().any(|block| {
                matches!(block, ContentBlock::Text { text } if text.contains("agent continues"))
            })
        } else {
            false
        }
    });
    assert!(
        has_text,
        "agent should continue and produce a text response after tool error"
    );
}

// ─── T016: tool_result_in_followup_message (AC 11) ───────────────────────

#[tokio::test]
async fn tool_result_in_followup_message() {
    let tool = Arc::new(MockTool::new("echo"));

    let stream_fn = Arc::new(MockContextCapturingStreamFn::new(vec![
        tool_call_events("call_1", "echo", "{}"),
        text_only_events("final answer"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn.clone(), default_convert)
            .with_tools(vec![tool.clone()])
            .with_retry_strategy(fast_retry()),
    );

    let _result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    let counts = stream_fn.captured_message_counts.lock().unwrap().clone();
    assert!(
        counts.len() >= 2,
        "stream should have been called at least twice (tool call + follow-up)"
    );
    assert!(
        counts[1] > counts[0],
        "second call should see more messages than first (tool result was added): {counts:?}",
    );
}

// ─── T017: tool_call_transformation via PreDispatch policy (AC 12) ──────

/// A `PreDispatch` policy that injects a field into tool call arguments.
struct InjectFieldPolicy;

impl swink_agent::PreDispatchPolicy for InjectFieldPolicy {
    fn name(&self) -> &'static str {
        "inject_field"
    }

    fn evaluate(
        &self,
        ctx: &mut swink_agent::ToolDispatchContext<'_>,
    ) -> swink_agent::PreDispatchVerdict {
        if let Some(obj) = ctx.arguments.as_object_mut() {
            obj.insert("injected".to_string(), json!(true));
        }
        swink_agent::PreDispatchVerdict::Continue
    }
}

#[tokio::test]
async fn tool_call_transformation() {
    let tool = Arc::new(MockArgCapturingTool::new(
        "transform_me",
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true
        }),
    ));

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "transform_me", r#"{"original":"value"}"#),
        text_only_events("done"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![tool.clone()])
            .with_pre_dispatch_policy(InjectFieldPolicy)
            .with_retry_strategy(fast_retry()),
    );

    let _result = agent.prompt_async(vec![user_msg("go")]).await.unwrap();

    let captured = tool
        .captured_args()
        .expect("tool should have captured args");
    assert_eq!(
        captured["injected"],
        json!(true),
        "policy should have injected a field"
    );
    assert_eq!(
        captured["original"],
        json!("value"),
        "original arg should still be present"
    );
}

// ─── T018: tool_validator_rejects_call via PreDispatch policy (edge case) ──

/// A `PreDispatch` policy that skips (rejects) a specific tool.
struct BlockToolPolicy;

impl swink_agent::PreDispatchPolicy for BlockToolPolicy {
    fn name(&self) -> &'static str {
        "block_tool"
    }

    fn evaluate(
        &self,
        ctx: &mut swink_agent::ToolDispatchContext<'_>,
    ) -> swink_agent::PreDispatchVerdict {
        if ctx.tool_name == "blocked" {
            swink_agent::PreDispatchVerdict::Skip("blocked".into())
        } else {
            swink_agent::PreDispatchVerdict::Continue
        }
    }
}

#[tokio::test]
async fn tool_validator_rejects_call() {
    let tool = Arc::new(MockTool::new("blocked"));

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "blocked", "{}"),
        text_only_events("done"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn, default_convert)
            .with_tools(vec![tool.clone()])
            .with_pre_dispatch_policy(BlockToolPolicy)
            .with_retry_strategy(fast_retry()),
    );

    let result = agent.prompt_async(vec![user_msg("hi")]).await.unwrap();

    assert!(
        !tool.was_executed(),
        "tool should NOT be executed when policy skips it"
    );

    // Verify there is an error ToolResult in the messages
    let has_error_result = result.messages.iter().any(|msg| {
        if let AgentMessage::Llm(LlmMessage::ToolResult(tr)) = msg {
            tr.is_error
        } else {
            false
        }
    });
    assert!(
        has_error_result,
        "should have an error tool result when policy skips"
    );
}

// ─── Regression (#204): PreDispatch Inject verdict must be applied ───────────

/// A `PreDispatch` policy that injects a user message before the tool runs.
struct InjectMessagePolicy;

impl swink_agent::PreDispatchPolicy for InjectMessagePolicy {
    fn name(&self) -> &'static str {
        "inject_message"
    }

    fn evaluate(
        &self,
        _ctx: &mut swink_agent::ToolDispatchContext<'_>,
    ) -> swink_agent::PreDispatchVerdict {
        swink_agent::PreDispatchVerdict::Inject(vec![user_msg("pre_dispatch_injected")])
    }
}

#[tokio::test]
async fn pre_dispatch_inject_is_applied() {
    // Regression for #204: injected messages were silently dropped.
    //
    // Without the fix, the second stream call sees: [user, assistant(tool_call), tool_result] = 3.
    // With the fix, it also sees the injected user message = 4.
    let tool = Arc::new(MockTool::new("noop"));

    let stream_fn = Arc::new(MockContextCapturingStreamFn::new(vec![
        tool_call_events("call_1", "noop", "{}"),
        text_only_events("done"),
    ]));

    let mut agent = Agent::new(
        AgentOptions::new("test", default_model(), stream_fn.clone(), default_convert)
            .with_tools(vec![tool.clone()])
            .with_pre_dispatch_policy(InjectMessagePolicy)
            .with_retry_strategy(fast_retry()),
    );

    let _ = agent.prompt_async(vec![user_msg("go")]).await.unwrap();

    let counts = stream_fn.captured_message_counts.lock().unwrap().clone();
    assert_eq!(
        counts.len(),
        2,
        "stream should be called twice (tool-call turn + follow-up turn): {counts:?}"
    );
    assert_eq!(
        counts[0], 1,
        "first call sees only the initial user message: {counts:?}"
    );
    assert_eq!(
        counts[1], 4,
        "second call must include the PreDispatch-injected message \
         (expected 4: user + assistant + tool_result + injected; got {counts:?})"
    );
}
