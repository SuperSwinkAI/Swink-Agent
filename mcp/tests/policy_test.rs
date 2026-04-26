//! Policy and approval tests for MCP tools (T026-T029).

mod common;

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use futures::stream::{self, Stream};
use serde_json::Value;
use swink_agent::{
    Agent, AgentEvent, AgentMessage, AgentOptions, AgentTool, ApprovalMode, AssistantMessageEvent,
    ContentBlock, Cost, LlmMessage, ModelSpec, PreDispatchPolicy, PreDispatchVerdict, StopReason,
    ToolApproval, ToolApprovalRequest, ToolDispatchContext, Usage,
};
use swink_agent_mcp::{McpConnection, McpServerConfig, McpTool, McpTransport};
use tokio_util::sync::CancellationToken;

type ApprovalCaptures = Arc<Mutex<Vec<(String, Option<Value>)>>>;

struct ScriptedStreamFn {
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
}

impl ScriptedStreamFn {
    fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

impl swink_agent::StreamFn for ScriptedStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a swink_agent::AgentContext,
        _options: &'a swink_agent::StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let events = self.responses.lock().unwrap().remove(0);
        Box::pin(stream::iter(events))
    }
}

struct BlockEchoPolicy;

impl PreDispatchPolicy for BlockEchoPolicy {
    fn name(&self) -> &'static str {
        "block_echo"
    }

    fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
        if ctx.tool_name == "echo" {
            PreDispatchVerdict::Skip("blocked by policy".into())
        } else {
            PreDispatchVerdict::Continue
        }
    }
}

fn llm_only(message: &AgentMessage) -> Option<LlmMessage> {
    match message {
        AgentMessage::Llm(message) => Some(message.clone()),
        AgentMessage::Custom(_) => None,
    }
}

fn tool_call_then_stop(id: &str, name: &str, args: &str) -> Vec<Vec<AssistantMessageEvent>> {
    vec![
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

async fn make_echo_tool(requires_approval: bool) -> Arc<dyn AgentTool> {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;
    let config = McpServerConfig {
        name: "policy-test-server".into(),
        transport: McpTransport::Stdio {
            command: "mock".into(),
            args: vec![],
            env: HashMap::default(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval,
        connect_timeout_ms: None,
        discovery_timeout_ms: None,
    };
    let conn = Arc::new(
        McpConnection::from_service(config, client, None)
            .await
            .unwrap(),
    );
    let echo_def = conn
        .discovered_tools
        .iter()
        .find(|tool| tool.name == "echo")
        .cloned()
        .unwrap();
    Arc::new(McpTool::new(
        &echo_def,
        None,
        "policy-test-server",
        requires_approval,
        conn,
    ))
}

fn collect_event_names(events: &[AgentEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            AgentEvent::ToolApprovalRequested { .. } => "ToolApprovalRequested",
            AgentEvent::ToolApprovalResolved { .. } => "ToolApprovalResolved",
            AgentEvent::ToolExecutionStart { .. } => "ToolExecutionStart",
            AgentEvent::ToolExecutionEnd { .. } => "ToolExecutionEnd",
            AgentEvent::McpToolCallStarted { .. } => "McpToolCallStarted",
            AgentEvent::McpToolCallCompleted { .. } => "McpToolCallCompleted",
            _ => "Other",
        })
        .collect()
}

/// Helper to create a disconnected `McpConnection` for metadata-only tests.
fn disconnected_connection(requires_approval: bool) -> (McpServerConfig, Arc<McpConnection>) {
    let config = McpServerConfig {
        name: "policy-test-server".into(),
        transport: McpTransport::Stdio {
            command: "mock".into(),
            args: vec![],
            env: HashMap::default(),
        },
        tool_prefix: None,
        tool_filter: None,
        requires_approval,
        connect_timeout_ms: None,
        discovery_timeout_ms: None,
    };
    let conn = Arc::new(McpConnection::disconnected(config.clone()));
    (config, conn)
}

/// T027: `McpTool` with `requires_approval=true` returns true from trait method.
#[tokio::test]
async fn mcp_tool_requires_approval_true_when_configured() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_def = tools.iter().find(|t| t.name == "echo").unwrap();

    let (_, conn) = disconnected_connection(true);
    let tool = McpTool::new(echo_def, None, "policy-test-server", true, conn);

    assert!(
        tool.requires_approval(),
        "requires_approval should be true when configured as true"
    );
}

/// T028: `McpTool` with `requires_approval=false` returns false.
#[tokio::test]
async fn mcp_tool_requires_approval_false_when_configured() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_def = tools.iter().find(|t| t.name == "echo").unwrap();

    let (_, conn) = disconnected_connection(false);
    let tool = McpTool::new(echo_def, None, "policy-test-server", false, conn);

    assert!(
        !tool.requires_approval(),
        "requires_approval should be false when configured as false"
    );
}

/// T029: `approval_context` returns the full params as context.
#[tokio::test]
async fn mcp_tool_approval_context_returns_params_for_policy_inspection() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_def = tools.iter().find(|t| t.name == "echo").unwrap();

    let (_, conn) = disconnected_connection(true);
    let tool = McpTool::new(echo_def, None, "policy-test-server", true, conn);

    let params = serde_json::json!({
        "text": "sensitive-input",
        "path": "/etc/passwd"
    });
    let context = tool.approval_context(&params);

    assert!(
        context.is_some(),
        "approval_context should return Some for MCP tools"
    );
    assert_eq!(
        context.unwrap(),
        params,
        "approval context should be the full params so policies can inspect arguments"
    );
}

/// Debug output must sanitize MCP approval context even though the raw params
/// stay available to approval and policy code.
#[tokio::test]
async fn mcp_tool_approval_context_is_redacted_in_tool_approval_request_debug() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_def = tools.iter().find(|t| t.name == "echo").unwrap();

    let (_, conn) = disconnected_connection(true);
    let tool = McpTool::new(echo_def, None, "policy-test-server", true, conn);

    let params = serde_json::json!({
        "Authorization": "Bearer top-secret",
        "path": "/tmp/output.txt",
        "text": "${API_KEY}"
    });
    let request = ToolApprovalRequest {
        tool_call_id: "call_1".into(),
        tool_name: tool.name().into(),
        arguments: params.clone(),
        requires_approval: true,
        context: tool.approval_context(&params),
    };

    let debug = format!("{request:?}");

    assert!(!debug.contains("top-secret"));
    assert!(!debug.contains("${API_KEY}"));
    assert!(debug.contains("/tmp/output.txt"));
    assert!(debug.contains("[REDACTED]"));
}

/// T026: `approval_context` is non-None — policies receive params for inspection.
/// Verifies the contract that MCP tools always expose params to approval/policy gates.
#[tokio::test]
async fn mcp_tool_always_provides_approval_context() {
    let config = common::MockServerConfig::new(vec![]);
    let client = common::spawn_mock_server_with_client(&config).await;
    let tools = client.peer().list_all_tools().await.unwrap();
    let echo_def = tools.iter().find(|t| t.name == "echo").unwrap();

    let (_, conn) = disconnected_connection(true);
    let tool = McpTool::new(echo_def, None, "policy-test-server", true, conn);

    // Empty params
    let empty = Value::Null;
    assert!(
        tool.approval_context(&empty).is_some(),
        "approval_context must be Some even for null params — policies must always be able to inspect MCP tool calls"
    );

    // Object params
    let obj = serde_json::json!({"key": "value"});
    assert!(
        tool.approval_context(&obj).is_some(),
        "approval_context must be Some for object params"
    );
}

#[tokio::test]
async fn approval_gate_fires_before_mcp_tool_execution() {
    let tool = make_echo_tool(true).await;
    let approvals: ApprovalCaptures = Arc::new(Mutex::new(Vec::new()));
    let approvals_ref = Arc::clone(&approvals);
    let events: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let events_ref = Arc::clone(&events);

    let stream_fn = Arc::new(ScriptedStreamFn::new(tool_call_then_stop(
        "call_1",
        "echo",
        r#"{"text":"hello from mcp"}"#,
    )));
    let options = AgentOptions::new("test", ModelSpec::new("test", "test"), stream_fn, llm_only)
        .with_tools(vec![tool])
        .with_approval_mode(ApprovalMode::Enabled)
        .with_approve_tool_async(move |req: ToolApprovalRequest| {
            let approvals_ref = Arc::clone(&approvals_ref);
            async move {
                approvals_ref
                    .lock()
                    .unwrap()
                    .push((req.tool_name, req.context));
                ToolApproval::Approved
            }
        })
        .with_event_forwarder(move |event| {
            events_ref.lock().unwrap().push(event);
        });
    let mut agent = Agent::new(options);

    let result = agent.prompt_text("hello").await.unwrap();

    let approvals = approvals.lock().unwrap();
    assert_eq!(approvals.len(), 1, "approval callback should fire once");
    assert_eq!(approvals[0].0, "echo");
    assert_eq!(
        approvals[0].1,
        Some(serde_json::json!({"text": "hello from mcp"}))
    );

    let event_names = collect_event_names(&events.lock().unwrap());
    let approval_idx = event_names
        .iter()
        .position(|name| *name == "ToolApprovalRequested")
        .expect("approval event");
    let execution_idx = event_names
        .iter()
        .position(|name| *name == "ToolExecutionStart")
        .expect("execution start event");
    assert!(
        approval_idx < execution_idx,
        "approval must precede execution, got: {event_names:?}"
    );

    let tool_texts: Vec<_> = result
        .messages
        .iter()
        .filter_map(|message| match message {
            AgentMessage::Llm(LlmMessage::ToolResult(result)) => {
                Some(ContentBlock::extract_text(&result.content))
            }
            _ => None,
        })
        .collect();
    assert!(
        tool_texts
            .iter()
            .any(|text| text.contains("hello from mcp")),
        "tool result should include the echoed MCP payload, got: {tool_texts:?}"
    );
}

#[tokio::test]
async fn pre_dispatch_policy_blocks_mcp_tool_before_approval_or_execution() {
    let tool = make_echo_tool(true).await;
    let approval_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let approval_calls_ref = Arc::clone(&approval_calls);
    let events: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let events_ref = Arc::clone(&events);

    let stream_fn = Arc::new(ScriptedStreamFn::new(tool_call_then_stop(
        "call_1",
        "echo",
        r#"{"text":"blocked"}"#,
    )));
    let options = AgentOptions::new("test", ModelSpec::new("test", "test"), stream_fn, llm_only)
        .with_tools(vec![tool])
        .with_approval_mode(ApprovalMode::Enabled)
        .with_pre_dispatch_policy(BlockEchoPolicy)
        .with_approve_tool_async(move |_req| {
            let approval_calls_ref = Arc::clone(&approval_calls_ref);
            async move {
                approval_calls_ref.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                ToolApproval::Approved
            }
        })
        .with_event_forwarder(move |event| {
            events_ref.lock().unwrap().push(event);
        });
    let mut agent = Agent::new(options);

    let result = agent.prompt_text("hello").await.unwrap();

    assert_eq!(
        approval_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "pre-dispatch skip should prevent the approval callback from firing"
    );

    let event_names = collect_event_names(&events.lock().unwrap());
    assert!(
        !event_names.contains(&"ToolExecutionStart"),
        "blocked tool must not execute, got events: {event_names:?}"
    );

    let has_policy_error = result.messages.iter().any(|message| match message {
        AgentMessage::Llm(LlmMessage::ToolResult(result)) => {
            result.is_error
                && ContentBlock::extract_text(&result.content).contains("blocked by policy")
        }
        _ => false,
    });
    assert!(
        has_policy_error,
        "blocked MCP tool should surface a policy error tool result"
    );
}
