//! Tests for the tool approval gate feature.

mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use common::{MockStreamFn, MockTool, default_convert, default_model, user_msg};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use swink_agent::{
    Agent, AgentEvent, AgentMessage, AgentOptions, AgentTool, AgentToolResult, ApprovalMode,
    AssistantMessageEvent, ContentBlock, Cost, LlmMessage, StopReason, ToolApproval, Usage,
};

// ─── Helpers ─────────────────────────────────────────────────────────────

fn tool_call_then_stop(id: &str, name: &str, args: &str) -> Vec<Vec<AssistantMessageEvent>> {
    vec![
        // Turn 1: tool call
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: id.to_string(),
                name: name.to_string(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: args.to_string(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ],
        // Turn 2: text response (after tool result)
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "done".to_string(),
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ],
    ]
}

fn make_agent(
    responses: Vec<Vec<AssistantMessageEvent>>,
    tools: Vec<Arc<dyn AgentTool>>,
) -> AgentOptions {
    AgentOptions::new(
        "test system prompt",
        default_model(),
        Arc::new(MockStreamFn::new(responses)),
        default_convert,
    )
    .with_tools(tools)
    .with_transform_context(|_msgs: &mut Vec<AgentMessage>, _overflow: bool| {})
}

// ─── Tests ───────────────────────────────────────────────────────────────

/// Test 1: No approval callback → tools execute immediately (backward compat).
#[tokio::test]
async fn no_approval_callback_tools_execute_normally() {
    let tool = Arc::new(MockTool::new("test_tool"));
    let tool_ref = Arc::clone(&tool);

    let responses = tool_call_then_stop("tc1", "test_tool", "{}");
    let options = make_agent(responses, vec![tool]);
    let mut agent = Agent::new(options);

    let result = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();
    assert!(result.error.is_none());
    assert!(tool_ref.was_executed());
}

/// Test 2: Always-approve callback → tools execute normally.
#[tokio::test]
async fn always_approve_callback_tools_execute() {
    let tool = Arc::new(MockTool::new("test_tool"));
    let tool_ref = Arc::clone(&tool);

    let responses = tool_call_then_stop("tc1", "test_tool", "{}");
    let options = make_agent(responses, vec![tool])
        .with_approve_tool(|_req| Box::pin(async { ToolApproval::Approved }));
    let mut agent = Agent::new(options);

    let result = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();
    assert!(result.error.is_none());
    assert!(tool_ref.was_executed());
}

/// Test 3: Always-reject callback → tools don't execute, LLM gets rejection error.
#[tokio::test]
async fn always_reject_callback_tools_not_executed() {
    let tool = Arc::new(MockTool::new("test_tool"));
    let tool_ref = Arc::clone(&tool);

    let responses = tool_call_then_stop("tc1", "test_tool", "{}");
    let options = make_agent(responses, vec![tool])
        .with_approve_tool(|_req| Box::pin(async { ToolApproval::Rejected }));
    let mut agent = Agent::new(options);

    let result = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();
    assert!(!tool_ref.was_executed(), "rejected tool should not execute");

    // The rejection error should appear as a tool result in the messages
    let has_rejection = result.messages.iter().any(|msg| {
        if let AgentMessage::Llm(LlmMessage::ToolResult(tr)) = msg {
            tr.is_error
                && tr
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("rejected")))
        } else {
            false
        }
    });
    assert!(has_rejection, "should contain rejection error tool result");
}

/// Test 4: Selective approval (approve by name) → correct tools run/rejected.
#[tokio::test]
async fn selective_approval_by_tool_name() {
    let allowed_tool = Arc::new(MockTool::new("allowed"));
    let blocked_tool = Arc::new(MockTool::new("blocked"));
    let allowed_ref = Arc::clone(&allowed_tool);
    let blocked_ref = Arc::clone(&blocked_tool);

    let responses = vec![
        // Turn 1: two tool calls
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: "tc1".to_string(),
                name: "allowed".to_string(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: "{}".to_string(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::ToolCallStart {
                content_index: 1,
                id: "tc2".to_string(),
                name: "blocked".to_string(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 1,
                delta: "{}".to_string(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 1 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ],
        // Turn 2: text stop
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "done".to_string(),
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ],
    ];

    let options =
        make_agent(responses, vec![allowed_tool, blocked_tool]).with_approve_tool(|req| {
            Box::pin(async move {
                if req.tool_name == "allowed" {
                    ToolApproval::Approved
                } else {
                    ToolApproval::Rejected
                }
            })
        });
    let mut agent = Agent::new(options);

    let result = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();
    assert!(allowed_ref.was_executed(), "allowed tool should execute");
    assert!(
        !blocked_ref.was_executed(),
        "blocked tool should not execute"
    );
    assert!(result.error.is_none());
}

/// Test 5: Events appear in correct order.
#[tokio::test]
async fn approval_events_in_correct_order() {
    let tool = Arc::new(MockTool::new("test_tool"));
    let events_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let responses = tool_call_then_stop("tc1", "test_tool", "{}");
    let options = make_agent(responses, vec![tool])
        .with_approve_tool(|_req| Box::pin(async { ToolApproval::Approved }));
    let mut agent = Agent::new(options);

    let log = Arc::clone(&events_log);
    agent.subscribe(move |event| {
        let name = match event {
            AgentEvent::ToolExecutionStart { .. } => "ToolExecutionStart",
            AgentEvent::ToolApprovalRequested { .. } => "ToolApprovalRequested",
            AgentEvent::ToolApprovalResolved { .. } => "ToolApprovalResolved",
            AgentEvent::ToolExecutionEnd { .. } => "ToolExecutionEnd",
            _ => return,
        };
        log.lock().unwrap().push(name.to_string());
    });

    let _result = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();

    let tool_events: Vec<String> = events_log.lock().unwrap().clone();

    assert_eq!(
        tool_events,
        vec![
            "ToolExecutionStart",
            "ToolApprovalRequested",
            "ToolApprovalResolved",
            "ToolExecutionEnd",
        ],
        "events should follow Start → ApprovalRequested → ApprovalResolved → End order"
    );
}

/// Test 6: Bypassed mode with callback set → callback never called, tools execute.
#[tokio::test]
async fn bypassed_mode_skips_approval_callback() {
    let tool = Arc::new(MockTool::new("test_tool"));
    let tool_ref = Arc::clone(&tool);
    let callback_called = Arc::new(AtomicBool::new(false));
    let callback_flag = Arc::clone(&callback_called);

    let responses = tool_call_then_stop("tc1", "test_tool", "{}");
    let options = make_agent(responses, vec![tool])
        .with_approve_tool(move |_req| {
            callback_flag.store(true, Ordering::SeqCst);
            Box::pin(async { ToolApproval::Rejected })
        })
        .with_approval_mode(ApprovalMode::Bypassed);
    let mut agent = Agent::new(options);

    let result = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();
    assert!(result.error.is_none());
    assert!(tool_ref.was_executed(), "tool should execute when bypassed");
    assert!(
        !callback_called.load(Ordering::SeqCst),
        "callback should never be called in Bypassed mode"
    );
}

// ─── Tests for requires_approval enhancement ────────────────────────────

/// Test 7: Default `requires_approval` is false.
#[test]
fn requires_approval_default_is_false() {
    let tool = MockTool::new("test");
    assert!(!tool.requires_approval());
}

/// Test 8: `BashTool` requires approval.
#[test]
fn requires_approval_bash_tool() {
    let tool = swink_agent::BashTool::new();
    assert!(tool.requires_approval());
}

/// Test 9: `WriteFileTool` requires approval.
#[test]
fn requires_approval_write_file_tool() {
    let tool = swink_agent::WriteFileTool::new();
    assert!(tool.requires_approval());
}

/// Test 10: `ReadFileTool` does not require approval.
#[test]
fn requires_approval_read_file_tool_is_false() {
    let tool = swink_agent::ReadFileTool::new();
    assert!(!tool.requires_approval());
}

/// Test 11: `selective_approve` auto-approves tools that don't require approval.
#[tokio::test]
async fn selective_approve_skips_non_requiring_tools() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use swink_agent::{ToolApproval, ToolApprovalRequest, selective_approve};

    let inner_called = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&inner_called);

    let wrapped = selective_approve(move |_req| {
        flag.store(true, Ordering::SeqCst);
        Box::pin(async { ToolApproval::Rejected }) // would reject if called
    });

    let req = ToolApprovalRequest {
        tool_call_id: "tc1".into(),
        tool_name: "safe_tool".into(),
        arguments: serde_json::json!({}),
        requires_approval: false,
    };

    let result = wrapped(req).await;
    assert_eq!(result, ToolApproval::Approved);
    assert!(!inner_called.load(Ordering::SeqCst));
}

/// Test 12: `selective_approve` delegates to inner callback for requiring tools.
#[tokio::test]
async fn selective_approve_calls_inner_for_requiring_tools() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use swink_agent::{ToolApproval, ToolApprovalRequest, selective_approve};

    let inner_called = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&inner_called);

    let wrapped = selective_approve(move |_req| {
        flag.store(true, Ordering::SeqCst);
        Box::pin(async { ToolApproval::Rejected })
    });

    let req = ToolApprovalRequest {
        tool_call_id: "tc1".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({}),
        requires_approval: true,
    };

    let result = wrapped(req).await;
    assert_eq!(result, ToolApproval::Rejected);
    assert!(inner_called.load(Ordering::SeqCst));
}

/// Test 13: approval request carries `requires_approval` field from tool.
#[tokio::test]
async fn approval_request_carries_requires_approval_field() {
    struct ApprovalRequiredTool {
        schema: Value,
    }

    impl ApprovalRequiredTool {
        fn new() -> Self {
            Self {
                schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": true
                }),
            }
        }
    }

    #[allow(clippy::unnecessary_literal_bound)]
    impl AgentTool for ApprovalRequiredTool {
        fn name(&self) -> &str {
            "danger_tool"
        }

        fn label(&self) -> &str {
            "Danger"
        }

        fn description(&self) -> &str {
            "A dangerous tool"
        }

        fn parameters_schema(&self) -> &Value {
            &self.schema
        }

        fn requires_approval(&self) -> bool {
            true
        }

        fn execute(
            &self,
            _tool_call_id: &str,
            _params: Value,
            _cancellation_token: CancellationToken,
            _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
            _state: std::sync::Arc<std::sync::RwLock<swink_agent::SessionState>>,
            _credential: Option<swink_agent::ResolvedCredential>,
        ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
            Box::pin(async { AgentToolResult::text("done") })
        }
    }

    let captured = Arc::new(Mutex::new(None::<bool>));
    let cap = Arc::clone(&captured);

    let responses = tool_call_then_stop("tc1", "danger_tool", "{}");
    let options = make_agent(responses, vec![Arc::new(ApprovalRequiredTool::new())])
        .with_approve_tool(move |req| {
            *cap.lock().unwrap() = Some(req.requires_approval);
            Box::pin(async { ToolApproval::Approved })
        });
    let mut agent = Agent::new(options);
    let _ = agent.prompt_async(vec![user_msg("hello")]).await.unwrap();

    assert_eq!(*captured.lock().unwrap(), Some(true));
}
