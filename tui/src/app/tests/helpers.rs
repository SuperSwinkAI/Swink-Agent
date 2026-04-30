use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use swink_agent::AgentTool;
use swink_agent::{
    Agent, AgentMessage, AgentOptions, AgentToolResult, AssistantMessage, Cost, LlmMessage,
    ModelSpec, StopReason, StreamFn, Usage, UserMessage, default_convert,
};
use tokio_util::sync::CancellationToken;

use super::super::*;

pub(super) fn make_test_agent(stream_fn: Arc<dyn StreamFn>) -> Agent {
    Agent::new(AgentOptions::new(
        "test system prompt",
        ModelSpec::new("test", "mock-model"),
        stream_fn,
        default_convert,
    ))
}

pub(super) fn make_test_agent_with_models(
    primary_model: ModelSpec,
    primary_stream_fn: Arc<dyn StreamFn>,
    extra_models: Vec<(ModelSpec, Arc<dyn StreamFn>)>,
) -> Agent {
    Agent::new(
        AgentOptions::new(
            "test system prompt",
            primary_model,
            primary_stream_fn,
            default_convert,
        )
        .with_available_models(extra_models),
    )
}

/// Drain all pending agent events from the channel, feeding them back
/// to the app (which in turn calls `agent.handle_stream_event`).
pub(super) fn drain_agent_events(app: &mut App) {
    while let Ok(event) = app.agent_rx.try_recv() {
        app.handle_agent_event(event);
    }
}

pub(super) async fn drain_agent_events_until_idle(app: &mut App) {
    loop {
        drain_agent_events(app);
        if app.status != AgentStatus::Running {
            break;
        }

        let event = tokio::time::timeout(Duration::from_secs(1), app.agent_rx.recv())
            .await
            .expect("agent should emit an event while running")
            .expect("agent event channel should stay open while running");
        app.handle_agent_event(event);
    }
}

pub(super) fn make_tool_result_message(content: &str) -> DisplayMessage {
    let summary = content
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(60)
        .collect::<String>();
    DisplayMessage {
        role: MessageRole::ToolResult,
        content: content.to_string(),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary,
        user_expanded: false,
        expanded_at: Some(Instant::now()),
        plan_mode: false,
        diff_data: None,
    }
}

pub(super) fn make_user_message(content: &str) -> DisplayMessage {
    DisplayMessage {
        role: MessageRole::User,
        content: content.to_string(),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary: String::new(),
        user_expanded: false,
        expanded_at: None,
        plan_mode: false,
        diff_data: None,
    }
}

pub(super) fn make_assistant_message(content: &str) -> DisplayMessage {
    DisplayMessage {
        role: MessageRole::Assistant,
        content: content.to_string(),
        thinking: None,
        is_streaming: false,
        collapsed: false,
        summary: String::new(),
        user_expanded: false,
        expanded_at: None,
        plan_mode: false,
        diff_data: None,
    }
}

pub(super) fn make_user_agent_message(content: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![swink_agent::ContentBlock::Text {
            text: content.to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

pub(super) fn make_assistant_agent_message(content: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
        content: vec![swink_agent::ContentBlock::Text {
            text: content.to_string(),
        }],
        provider: "test".to_string(),
        model_id: "mock-model".to_string(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    }))
}

pub(super) fn make_error_assistant_agent_message(error_msg: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
        content: vec![],
        provider: "test".to_string(),
        model_id: "mock-model".to_string(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::Error,
        error_message: Some(error_msg.to_string()),
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    }))
}

pub(super) fn instant_secs_ago(secs: u64) -> Instant {
    Instant::now()
        .checked_sub(Duration::from_secs(secs))
        .unwrap()
}

pub(super) struct MockReadTool;

impl AgentTool for MockReadTool {
    fn name(&self) -> &'static str {
        "read_file"
    }
    fn label(&self) -> &'static str {
        "Read File"
    }
    fn description(&self) -> &'static str {
        "Read a file"
    }
    fn parameters_schema(&self) -> &serde_json::Value {
        &serde_json::Value::Null
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
        Box::pin(async { AgentToolResult::text("ok") })
    }
}

pub(super) struct MockWriteTool;

impl AgentTool for MockWriteTool {
    fn name(&self) -> &'static str {
        "write_file"
    }
    fn label(&self) -> &'static str {
        "Write File"
    }
    fn description(&self) -> &'static str {
        "Write a file"
    }
    fn parameters_schema(&self) -> &serde_json::Value {
        &serde_json::Value::Null
    }
    fn requires_approval(&self) -> bool {
        true
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
        Box::pin(async { AgentToolResult::text("ok") })
    }
}

pub(super) fn make_test_agent_with_tools(stream_fn: Arc<dyn StreamFn>) -> Agent {
    let mut agent = Agent::new(AgentOptions::new(
        "test system prompt",
        ModelSpec::new("test", "mock-model"),
        stream_fn,
        default_convert,
    ));
    agent.set_tools(vec![
        Arc::new(MockReadTool) as Arc<dyn AgentTool>,
        Arc::new(MockWriteTool) as Arc<dyn AgentTool>,
    ]);
    agent
}
