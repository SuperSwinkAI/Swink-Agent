use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use swink_agent::AgentTool;
use swink_agent::testing::text_only_events;
use swink_agent::{
    Agent, AgentContext, AgentMessage, AgentOptions, AgentToolResult, AssistantMessage,
    AssistantMessageEvent, LlmMessage, ModelSpec, StopReason, StreamFn, StreamOptions, UserMessage,
    default_convert,
};
use tokio_util::sync::CancellationToken;

use super::super::*;

/// A `StreamFn` that records the user text of every prompt it is streamed.
///
/// This is how we assert on what actually reached the model, rather than
/// trusting the TUI's own bookkeeping. Shared by the `@path` mention tests and
/// the `/skill` tests.
pub(super) struct PromptCapturingStreamFn {
    pub(super) prompts: Arc<Mutex<Vec<String>>>,
}

impl StreamFn for PromptCapturingStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn futures::Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let texts: Vec<String> = context
            .messages
            .iter()
            .filter_map(|message| match message {
                swink_agent::AgentMessage::Llm(swink_agent::LlmMessage::User(user)) => {
                    Some(user.content.iter().filter_map(|block| match block {
                        swink_agent::ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    }))
                }
                _ => None,
            })
            .flatten()
            .collect();
        self.prompts.lock().unwrap().extend(texts);
        Box::pin(futures::stream::iter(text_only_events("ok")))
    }
}

/// Counts resolver invocations and records the text each one saw.
#[derive(Default)]
pub(super) struct ResolverSpy {
    pub(super) calls: AtomicUsize,
    pub(super) seen: Mutex<Vec<String>>,
}

impl ResolverSpy {
    pub(super) fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

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

/// Drain all pending agent events from the transport, feeding them back
/// to the app (which in turn calls `agent.handle_stream_event`).
pub(super) fn drain_agent_events(app: &mut App) {
    while let Some(event) = app.agent_io.transport.try_recv() {
        app.handle_agent_event(event);
    }
}

pub(super) async fn drain_agent_events_until_idle(app: &mut App) {
    loop {
        drain_agent_events(app);
        if app.agent_io.status != AgentStatus::Running {
            break;
        }

        let event = tokio::time::timeout(Duration::from_secs(1), app.agent_io.transport.recv())
            .await
            .expect("agent should emit an event while running")
            .expect("agent event stream should stay open while running");
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
    AgentMessage::Llm(LlmMessage::User(
        UserMessage::new(vec![swink_agent::ContentBlock::Text {
            text: content.to_string(),
        }])
        .with_timestamp(0),
    ))
}

pub(super) fn make_assistant_agent_message(content: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::Assistant(
        AssistantMessage::new(
            vec![swink_agent::ContentBlock::Text {
                text: content.to_string(),
            }],
            "test",
            "mock-model",
        )
        .with_stop_reason(StopReason::Stop)
        .with_timestamp(0),
    ))
}

pub(super) fn make_error_assistant_agent_message(error_msg: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::Assistant(
        AssistantMessage::new(vec![], "test", "mock-model")
            .with_stop_reason(StopReason::Error)
            .with_error_message(error_msg)
            .with_timestamp(0),
    ))
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
