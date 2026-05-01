#![cfg(feature = "testkit")]
mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    MockApiKeyCapturingStreamFn, MockContextCapturingStreamFn, MockStreamFn, MockTool,
    default_model, text_only_events, tool_call_events,
};
use futures::Stream;
use futures::stream::StreamExt;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AgentEvent, AgentLoopConfig, AgentMessage, AgentTool, AgentToolResult,
    AssistantMessage, AssistantMessageEvent, ContentBlock, Cost, CustomMessage,
    DefaultRetryStrategy, LlmMessage, MessageProvider, ModelSpec, PolicyContext, PolicyVerdict,
    PostTurnPolicy, PreTurnPolicy, StopReason, StreamFn, StreamOptions, ToolResultMessage,
    TurnPolicyContext, TurnSnapshot, Usage, UserMessage, agent_loop, agent_loop_continue,
};

// ─── MockUpdatingTool ─────────────────────────────────────────────────────────

struct MockUpdatingTool {
    tool_name: String,
}

impl MockUpdatingTool {
    fn new(name: &str) -> Self {
        Self {
            tool_name: name.to_string(),
        }
    }
}

impl AgentTool for MockUpdatingTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn label(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &'static str {
        "A tool that emits partial updates"
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
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::ResolvedCredential>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async move {
            if let Some(on_update) = on_update {
                on_update(AgentToolResult::text("partial-1"));
                on_update(AgentToolResult::text("partial-2"));
            }
            AgentToolResult::text("final")
        })
    }
}

struct MockCancellationIgnoringTool {
    tool_name: String,
}

impl MockCancellationIgnoringTool {
    fn new(name: &str) -> Self {
        Self {
            tool_name: name.to_string(),
        }
    }
}

impl AgentTool for MockCancellationIgnoringTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn label(&self) -> &str {
        &self.tool_name
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
        Box::pin(async move { std::future::pending::<AgentToolResult>().await })
    }
}

struct CancelsOnTextStartStreamFn;

impl StreamFn for CancelsOnTextStartStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a AgentContext,
        _options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "unreachable".to_string(),
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];

        Box::pin(futures::stream::iter(events).inspect(move |event| {
            if matches!(event, AssistantMessageEvent::TextStart { .. }) {
                cancellation_token.cancel();
            }
        }))
    }
}

// ─── Helper functions ────────────────────────────────────────────────────

/// Test helper that delegates to closures for steering/follow-up.
///
/// Steering messages are stored in an internal queue so that `has_steering`
/// can peek non-destructively while `poll_steering` drains. Follow-up uses
/// the original closure-delegation model.
struct MockMessageProvider {
    steering_queue: Arc<Mutex<std::collections::VecDeque<AgentMessage>>>,
    refill_steering: Option<Box<dyn Fn() -> Vec<AgentMessage> + Send + Sync>>,
    follow_up: Box<dyn Fn() -> Vec<AgentMessage> + Send + Sync>,
}

impl MockMessageProvider {
    fn steering_only(f: impl Fn() -> Vec<AgentMessage> + Send + Sync + 'static) -> Self {
        Self {
            steering_queue: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            refill_steering: Some(Box::new(f)),
            follow_up: Box::new(Vec::new),
        }
    }

    fn follow_up_only(f: impl Fn() -> Vec<AgentMessage> + Send + Sync + 'static) -> Self {
        Self {
            steering_queue: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            refill_steering: None,
            follow_up: Box::new(f),
        }
    }

    /// Refill the internal steering queue from the refill closure (if any).
    ///
    /// Called lazily from both `has_steering` (for non-destructive peek) and
    /// `poll_steering` (for drain) so the closure drives whether messages
    /// become available — mirroring the old behaviour while keeping the two
    /// methods consistent.
    fn refill(&self) {
        if let Some(ref f) = self.refill_steering {
            let msgs = f();
            if !msgs.is_empty() {
                let mut guard = self
                    .steering_queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.extend(msgs);
            }
        }
    }
}

impl MessageProvider for MockMessageProvider {
    fn poll_steering(&self) -> Vec<AgentMessage> {
        self.refill();
        let mut guard = self
            .steering_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.drain(..).collect()
    }

    fn poll_follow_up(&self) -> Vec<AgentMessage> {
        (self.follow_up)()
    }

    fn has_steering(&self) -> bool {
        self.refill();
        let guard = self
            .steering_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !guard.is_empty()
    }
}

type ConvertToLlmBoxed = Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync>;

fn default_convert_to_llm() -> ConvertToLlmBoxed {
    Box::new(|msg: &AgentMessage| match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    })
}

fn default_config(stream_fn: Arc<dyn StreamFn>) -> AgentLoopConfig {
    AgentLoopConfig {
        agent_name: None,
        transfer_chain: None,
        model: default_model(),
        stream_options: StreamOptions::default(),
        retry_strategy: Box::new(
            DefaultRetryStrategy::default()
                .with_jitter(false)
                .with_base_delay(Duration::from_millis(1)),
        ),
        stream_fn,
        tools: vec![],
        convert_to_llm: default_convert_to_llm(),
        transform_context: None,
        get_api_key: None,
        message_provider: None,
        pending_message_snapshot: Arc::default(),
        loop_context_snapshot: Arc::default(),
        approve_tool: None,
        approval_mode: swink_agent::ApprovalMode::default(),
        pre_turn_policies: vec![],
        pre_dispatch_policies: vec![],
        post_turn_policies: vec![],
        post_loop_policies: vec![],
        async_transform_context: None,
        metrics_collector: None,
        fallback: None,
        tool_execution_policy: swink_agent::ToolExecutionPolicy::default(),
        session_state: std::sync::Arc::new(
            std::sync::RwLock::new(swink_agent::SessionState::new()),
        ),
        credential_resolver: None,
        cache_config: None,
        cache_state: std::sync::Mutex::new(swink_agent::CacheState::default()),
        dynamic_system_prompt: None,
    }
}

fn terminal_done_events(text: &str, stop_reason: StopReason) -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: text.to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ]
}

/// Collect all events from a loop stream.
async fn collect_events(stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>) -> Vec<AgentEvent> {
    stream.collect().await
}

/// Collect events until the loop reports its terminal event.
async fn collect_events_until_agent_end(
    mut stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>,
) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        let is_agent_end = matches!(event, AgentEvent::AgentEnd { .. });
        events.push(event);
        if is_agent_end {
            break;
        }
    }
    events
}

/// Check if events contain a specific variant (by Debug name prefix).
fn has_event(events: &[AgentEvent], name: &str) -> bool {
    events.iter().any(|e| format!("{e:?}").starts_with(name))
}

fn count_events(events: &[AgentEvent], name: &str) -> usize {
    events
        .iter()
        .filter(|e| format!("{e:?}").starts_with(name))
        .count()
}

struct StoppingPostTurnPolicy;

impl PostTurnPolicy for StoppingPostTurnPolicy {
    fn name(&self) -> &'static str {
        "stopping-post-turn"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, _turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        PolicyVerdict::Stop("budget exceeded".to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedTurnContext {
    message_count: usize,
    tool_result_count: usize,
    last_message_kind: &'static str,
}

struct RecordingPostTurnPolicy {
    observations: Arc<Mutex<Vec<RecordedTurnContext>>>,
}

impl PostTurnPolicy for RecordingPostTurnPolicy {
    fn name(&self) -> &'static str {
        "recording-post-turn"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let last_message_kind = match turn.context_messages.last() {
            Some(AgentMessage::Llm(LlmMessage::Assistant(_))) => "assistant",
            Some(AgentMessage::Llm(LlmMessage::ToolResult(_))) => "tool_result",
            Some(AgentMessage::Llm(LlmMessage::User(_))) => "user",
            Some(AgentMessage::Custom(_)) => "custom",
            None => "none",
        };

        self.observations.lock().unwrap().push(RecordedTurnContext {
            message_count: turn.context_messages.len(),
            tool_result_count: turn.tool_results.len(),
            last_message_kind,
        });

        PolicyVerdict::Continue
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RecordedPreTurnBatch {
    turn_index: usize,
    message_count: usize,
    new_messages: Vec<String>,
}

struct RecordingPreTurnPolicy {
    observations: Arc<Mutex<Vec<RecordedPreTurnBatch>>>,
}

impl PreTurnPolicy for RecordingPreTurnPolicy {
    fn name(&self) -> &'static str {
        "recording-pre-turn"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>) -> PolicyVerdict {
        let new_messages = ctx
            .new_messages
            .iter()
            .filter_map(|message| match message {
                AgentMessage::Llm(LlmMessage::User(user)) => {
                    Some(ContentBlock::extract_text(&user.content))
                }
                AgentMessage::Llm(LlmMessage::Assistant(assistant)) => {
                    Some(ContentBlock::extract_text(&assistant.content))
                }
                AgentMessage::Llm(LlmMessage::ToolResult(result)) => {
                    Some(ContentBlock::extract_text(&result.content))
                }
                AgentMessage::Custom(_) => None,
            })
            .collect();
        self.observations
            .lock()
            .unwrap()
            .push(RecordedPreTurnBatch {
                turn_index: ctx.turn_index,
                message_count: ctx.message_count,
                new_messages,
            });
        PolicyVerdict::Continue
    }
}

struct InjectingOncePostTurnPolicy {
    injected: AtomicBool,
    text: String,
}

struct InjectingOncePreTurnPolicy {
    injected: AtomicBool,
    text: String,
}

impl PreTurnPolicy for InjectingOncePreTurnPolicy {
    fn name(&self) -> &'static str {
        "injecting-once-pre-turn"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>) -> PolicyVerdict {
        if self.injected.swap(true, Ordering::SeqCst) {
            PolicyVerdict::Continue
        } else {
            PolicyVerdict::Inject(vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: self.text.clone(),
                }],
                timestamp: 0,
                cache_hint: None,
            }))])
        }
    }
}

impl PostTurnPolicy for InjectingOncePostTurnPolicy {
    fn name(&self) -> &'static str {
        "injecting-once-post-turn"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, _turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        if self.injected.swap(true, Ordering::SeqCst) {
            PolicyVerdict::Continue
        } else {
            PolicyVerdict::Inject(vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: self.text.clone(),
                }],
                timestamp: 0,
                cache_hint: None,
            }))])
        }
    }
}

struct MockTransferTool {
    tool_name: String,
    target_agent: String,
    reason: String,
}

impl MockTransferTool {
    fn new(name: &str, target_agent: &str, reason: &str) -> Self {
        Self {
            tool_name: name.to_string(),
            target_agent: target_agent.to_string(),
            reason: reason.to_string(),
        }
    }
}

impl AgentTool for MockTransferTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn label(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &'static str {
        "A tool that always requests an agent transfer"
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
        let signal = swink_agent::TransferSignal::new(&self.target_agent, &self.reason);
        Box::pin(async move { AgentToolResult::transfer(signal) })
    }
}

// ─── 3.1: Single-turn no-tool ────────────────────────────────────────────

#[tokio::test]
async fn single_turn_no_tool() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("Hello!")]));
    let config = default_config(stream_fn);

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentStart"));
    assert!(has_event(&events, "TurnStart"));
    assert!(has_event(&events, "MessageStart"));
    assert!(has_event(&events, "MessageUpdate"));
    assert!(has_event(&events, "MessageEnd"));
    assert!(has_event(&events, "TurnEnd"));
    assert!(has_event(&events, "AgentEnd"));

    let names: Vec<String> = events
        .iter()
        .map(|e| {
            let s = format!("{e:?}");
            s.split([' ', '{', '(']).next().unwrap_or("").to_string()
        })
        .collect();

    let agent_start_idx = names.iter().position(|n| n == "AgentStart").unwrap();
    let turn_start_idx = names.iter().position(|n| n == "TurnStart").unwrap();
    let msg_start_idx = names.iter().position(|n| n == "MessageStart").unwrap();
    let msg_update_idx = names.iter().position(|n| n == "MessageUpdate").unwrap();
    let msg_end_idx = names.iter().position(|n| n == "MessageEnd").unwrap();
    let turn_end_idx = names.iter().position(|n| n == "TurnEnd").unwrap();
    let agent_end_idx = names.iter().position(|n| n == "AgentEnd").unwrap();

    assert!(agent_start_idx < turn_start_idx);
    assert!(turn_start_idx < msg_start_idx);
    assert!(msg_start_idx < msg_update_idx);
    assert!(msg_update_idx < msg_end_idx);
    assert!(msg_end_idx < turn_end_idx);
    assert!(turn_end_idx < agent_end_idx);
}

// ─── 3.2: Single-turn with tool call ─────────────────────────────────────

#[tokio::test]
async fn single_turn_with_tool_call() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "read_file", r#"{"path": "/tmp"}"#),
        text_only_events("Done!"),
    ]));

    let tool = Arc::new(MockTool::new("read_file"));
    let mut config = default_config(stream_fn);
    config.tools = vec![tool];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "ToolExecutionStart"));
    assert!(has_event(&events, "ToolExecutionEnd"));
    assert_eq!(count_events(&events, "TurnStart"), 2);
    assert_eq!(count_events(&events, "TurnEnd"), 2);
}

// ─── 3.3: Multi-turn ────────────────────────────────────────────────────

#[tokio::test]
async fn multi_turn() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "tool_a", "{}"),
        tool_call_events("tc_2", "tool_b", "{}"),
        text_only_events("Final answer"),
    ]));

    let tool_a = Arc::new(MockTool::new("tool_a"));
    let tool_b = Arc::new(MockTool::new("tool_b"));
    let mut config = default_config(stream_fn);
    config.tools = vec![tool_a, tool_b];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert_eq!(count_events(&events, "TurnStart"), 3);
    assert_eq!(count_events(&events, "TurnEnd"), 3);
    assert!(has_event(&events, "AgentEnd"));
}

// ─── 3.4: transform_context ordering ─────────────────────────────────────

#[tokio::test]
async fn transform_context_ordering() {
    let counter = Arc::new(AtomicU32::new(0));
    let counter_transform = Arc::clone(&counter);
    let counter_convert = Arc::clone(&counter);

    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("ok")]));
    let mut config = default_config(stream_fn);

    config.transform_context = Some(Arc::new(
        move |_msgs: &mut Vec<AgentMessage>, _overflow: bool| {
            counter_transform.fetch_add(1, Ordering::SeqCst);
        },
    ));

    config.convert_to_llm = Box::new(move |msg: &AgentMessage| {
        let val = counter_convert.load(Ordering::SeqCst);
        assert!(
            val > 0,
            "transform_context should run before convert_to_llm"
        );
        match msg {
            AgentMessage::Llm(llm) => Some(llm.clone()),
            AgentMessage::Custom(_) => None,
        }
    });

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    assert!(counter.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn pre_turn_policy_keeps_fresh_batch_when_transformer_rewrites_context() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("ok")]));
    let observations = Arc::new(Mutex::new(Vec::new()));

    let mut config = default_config(stream_fn);
    config.transform_context = Some(Arc::new(
        move |msgs: &mut Vec<AgentMessage>, _overflow: bool| {
            if let Some(AgentMessage::Llm(LlmMessage::User(user))) = msgs.first_mut() {
                user.content = vec![ContentBlock::Text {
                    text: "transformed prompt".to_string(),
                }];
            }
        },
    ));
    config.pre_turn_policies = vec![Arc::new(RecordingPreTurnPolicy {
        observations: Arc::clone(&observations),
    })];

    let events = collect_events(agent_loop(
        vec![common::user_msg("original prompt")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    let recorded = observations.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1, "pre-turn policy should run once");
    assert_eq!(
        recorded[0],
        RecordedPreTurnBatch {
            turn_index: 0,
            message_count: 1,
            new_messages: vec!["original prompt".to_string()],
        },
        "pre-turn policies should inspect the immutable fresh batch"
    );

    let before_llm_messages = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::BeforeLlmCall { messages, .. } => Some(messages),
            _ => None,
        })
        .expect("provider call should emit BeforeLlmCall");
    let provider_text = before_llm_messages
        .first()
        .and_then(|message| match message {
            LlmMessage::User(user) => Some(ContentBlock::extract_text(&user.content)),
            _ => None,
        })
        .expect("provider input should include transformed user text");
    assert_eq!(
        provider_text, "transformed prompt",
        "transformers should still affect the provider-bound context"
    );
}

#[tokio::test]
async fn pre_turn_new_messages_survive_context_compaction() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("ok")]));
    let observations = Arc::new(Mutex::new(Vec::new()));

    let mut config = default_config(stream_fn);
    config.transform_context = Some(Arc::new(
        move |msgs: &mut Vec<AgentMessage>, _overflow: bool| {
            if !msgs.is_empty() {
                msgs.remove(0);
            }
        },
    ));
    config.pre_turn_policies = vec![Arc::new(RecordingPreTurnPolicy {
        observations: Arc::clone(&observations),
    })];

    let events = collect_events(agent_loop_continue(
        vec![
            common::user_msg("prior conversation"),
            common::user_msg("fresh prompt"),
        ],
        1,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    let recorded = observations.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1, "pre-turn policy should run once");
    assert_eq!(
        recorded[0],
        RecordedPreTurnBatch {
            turn_index: 0,
            message_count: 1,
            new_messages: vec!["fresh prompt".to_string()],
        },
        "compaction must not make the fresh pre-turn batch empty or polluted"
    );
}

#[tokio::test]
async fn pre_turn_new_messages_exclude_transformer_appends() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("ok")]));
    let observations = Arc::new(Mutex::new(Vec::new()));

    let mut config = default_config(stream_fn);
    config.transform_context = Some(Arc::new(
        move |msgs: &mut Vec<AgentMessage>, _overflow: bool| {
            msgs.push(common::user_msg("retrieved context"));
        },
    ));
    config.pre_turn_policies = vec![Arc::new(RecordingPreTurnPolicy {
        observations: Arc::clone(&observations),
    })];

    let events = collect_events(agent_loop_continue(
        vec![
            common::user_msg("prior conversation"),
            common::user_msg("fresh prompt"),
        ],
        1,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    let recorded = observations.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1, "pre-turn policy should run once");
    assert_eq!(
        recorded[0],
        RecordedPreTurnBatch {
            turn_index: 0,
            message_count: 3,
            new_messages: vec!["fresh prompt".to_string()],
        },
        "transformer-appended context must not replace the fresh pre-turn batch"
    );
}

#[tokio::test]
async fn pre_turn_injections_reach_imminent_provider_call() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("ok")]));
    let mut config = default_config(stream_fn);
    config.pre_turn_policies = vec![Arc::new(InjectingOncePreTurnPolicy {
        injected: AtomicBool::new(false),
        text: "policy context".to_string(),
    })];

    let events = collect_events(agent_loop(
        vec![common::user_msg("start")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    let before_llm_messages = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::BeforeLlmCall { messages, .. } => Some(messages),
            _ => None,
        })
        .expect("provider call should emit BeforeLlmCall");
    let input_text: Vec<String> = before_llm_messages
        .iter()
        .filter_map(|message| match message {
            LlmMessage::User(user) => Some(ContentBlock::extract_text(&user.content)),
            _ => None,
        })
        .collect();
    assert_eq!(
        input_text,
        vec!["start".to_string(), "policy context".to_string()],
        "pre-turn injected messages must be present in the current provider input"
    );
}

// ─── 3.5: get_api_key ────────────────────────────────────────────────────

#[tokio::test]
async fn get_api_key() {
    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let calls_clone = Arc::clone(&calls);

    let stream_fn = Arc::new(MockApiKeyCapturingStreamFn::new(vec![
        tool_call_events("tc_1", "tool_a", "{}"),
        text_only_events("done"),
    ]));
    let api_key_captures = Arc::clone(&stream_fn);

    let tool = Arc::new(MockTool::new("tool_a"));
    let mut config = default_config(stream_fn);
    config.tools = vec![tool];
    config.get_api_key = Some(Box::new(move |provider: &str| {
        let calls = Arc::clone(&calls_clone);
        let provider = provider.to_string();
        Box::pin(async move {
            calls.lock().unwrap().push(provider);
            Some("key-123".to_string())
        })
    }));

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    let recorded = calls.lock().unwrap();
    assert!(
        recorded.len() >= 2,
        "get_api_key should be called on each turn, got {} calls",
        recorded.len()
    );
    assert!(recorded.iter().all(|p| p == "test"));
    drop(recorded);

    let captured_api_keys = api_key_captures.captured_api_keys.lock().unwrap();
    assert!(
        captured_api_keys
            .iter()
            .all(|key| key.as_deref() == Some("key-123")),
        "resolved API key should be forwarded on every turn: {captured_api_keys:?}"
    );
    drop(captured_api_keys);
}

#[tokio::test]
async fn tool_execution_update_events() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "updating_tool", "{}"),
        text_only_events("done"),
    ]));

    let tool = Arc::new(MockUpdatingTool::new("updating_tool"));
    let mut config = default_config(stream_fn);
    config.tools = vec![tool];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    let names: Vec<String> = events
        .iter()
        .map(|event| {
            format!("{event:?}")
                .split([' ', '{', '('])
                .next()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    let tool_start_idx = names
        .iter()
        .position(|n| n == "ToolExecutionStart")
        .expect("ToolExecutionStart");
    let first_update_idx = names
        .iter()
        .position(|n| n == "ToolExecutionUpdate")
        .expect("ToolExecutionUpdate");
    let tool_end_idx = names
        .iter()
        .position(|n| n == "ToolExecutionEnd")
        .expect("ToolExecutionEnd");
    assert!(tool_start_idx < first_update_idx);
    assert!(first_update_idx < tool_end_idx);

    let partials: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionUpdate { id, name, partial } => {
                assert_eq!(id, "tc_1");
                assert_eq!(name, "updating_tool");
                Some(ContentBlock::extract_text(&partial.content))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        partials,
        vec!["partial-1".to_string(), "partial-2".to_string()]
    );

    let final_tool_result = events.iter().find_map(|event| match event {
        AgentEvent::TurnEnd { tool_results, .. } => Some(tool_results.clone()),
        _ => None,
    });
    let final_tool_result = final_tool_result.expect("turn end with tool result");
    assert_eq!(final_tool_result.len(), 1);
    assert_eq!(
        ContentBlock::extract_text(&final_tool_result[0].content),
        "final"
    );
    assert!(
        !final_tool_result[0]
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text } if text.contains("partial"))),
        "partial updates must not leak into final tool results"
    );
}

// ─── 3.6: Concurrent execution ──────────────────────────────────────────

#[tokio::test]
async fn concurrent_execution() {
    let events_with_3_tools = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_1".to_string(),
            name: "slow_tool".to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "{}".to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
        AssistantMessageEvent::ToolCallStart {
            content_index: 1,
            id: "tc_2".to_string(),
            name: "slow_tool2".to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 1,
            delta: "{}".to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 1 },
        AssistantMessageEvent::ToolCallStart {
            content_index: 2,
            id: "tc_3".to_string(),
            name: "slow_tool3".to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 2,
            delta: "{}".to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 2 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ];

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        events_with_3_tools,
        text_only_events("done"),
    ]));

    let delay = Duration::from_millis(100);
    let tool1 = Arc::new(MockTool::new("slow_tool").with_delay(delay));
    let tool2 = Arc::new(MockTool::new("slow_tool2").with_delay(delay));
    let tool3 = Arc::new(MockTool::new("slow_tool3").with_delay(delay));

    let mut config = default_config(stream_fn);
    config.tools = vec![tool1, tool2, tool3];

    let start = std::time::Instant::now();
    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;
    let elapsed = start.elapsed();

    assert!(has_event(&events, "AgentEnd"));
    assert!(
        elapsed < Duration::from_millis(500),
        "tools should execute concurrently, took {elapsed:?}"
    );
}

// ─── 3.7: Steering interrupt ─────────────────────────────────────────────

#[tokio::test]
async fn steering_interrupt() {
    let events_with_2_tools = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_1".to_string(),
            name: "fast_tool".to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "{}".to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
        AssistantMessageEvent::ToolCallStart {
            content_index: 1,
            id: "tc_2".to_string(),
            name: "slow_tool".to_string(),
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
    ];

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        events_with_2_tools,
        text_only_events("after steering"),
    ]));

    let fast_tool = Arc::new(MockTool::new("fast_tool").with_delay(Duration::from_millis(10)));
    let slow_tool = Arc::new(MockTool::new("slow_tool").with_delay(Duration::from_millis(500)));

    let steering_call_count = Arc::new(AtomicU32::new(0));
    let steering_count_clone = Arc::clone(&steering_call_count);

    let mut config = default_config(stream_fn);
    config.tools = vec![fast_tool, slow_tool];
    config.message_provider = Some(Arc::new(MockMessageProvider::steering_only(move || {
        let count = steering_count_clone.fetch_add(1, Ordering::SeqCst);
        if count == 0 {
            vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "steering: change direction".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            }))]
        } else {
            vec![]
        }
    })));

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    assert!(has_event(&events, "ToolExecutionStart"));
}

#[tokio::test]
async fn steering_interrupt_aborts_cancellation_unaware_tools() {
    let events_with_2_tools = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_1".to_string(),
            name: "fast_tool".to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "{}".to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
        AssistantMessageEvent::ToolCallStart {
            content_index: 1,
            id: "tc_2".to_string(),
            name: "stuck_tool".to_string(),
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
    ];

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        events_with_2_tools,
        text_only_events("after steering"),
    ]));

    let fast_tool = Arc::new(MockTool::new("fast_tool").with_delay(Duration::from_millis(10)));
    let stuck_tool = Arc::new(MockCancellationIgnoringTool::new("stuck_tool"));

    let steering_call_count = Arc::new(AtomicU32::new(0));
    let steering_count_clone = Arc::clone(&steering_call_count);

    let mut config = default_config(stream_fn);
    config.tools = vec![fast_tool, stuck_tool];
    config.message_provider = Some(Arc::new(MockMessageProvider::steering_only(move || {
        let count = steering_count_clone.fetch_add(1, Ordering::SeqCst);
        if count == 0 {
            vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "steering: change direction".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            }))]
        } else {
            vec![]
        }
    })));

    let events = collect_events_until_agent_end(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    assert_eq!(count_events(&events, "TurnStart"), 2);
    assert_eq!(count_events(&events, "ToolExecutionEnd"), 1);
}

// ─── 3.8: Follow-up ─────────────────────────────────────────────────────

#[tokio::test]
async fn follow_up() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first response"),
        text_only_events("second response"),
    ]));

    let follow_up_count = Arc::new(AtomicU32::new(0));
    let follow_up_clone = Arc::clone(&follow_up_count);

    let mut config = default_config(stream_fn);
    config.message_provider = Some(Arc::new(MockMessageProvider::follow_up_only(move || {
        let count = follow_up_clone.fetch_add(1, Ordering::SeqCst);
        if count == 0 {
            vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "follow up question".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            }))]
        } else {
            vec![]
        }
    })));

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert_eq!(count_events(&events, "TurnStart"), 2);
    assert!(has_event(&events, "AgentEnd"));
}

// ─── 3.9: Error exit ─────────────────────────────────────────────────────

#[tokio::test]
async fn error_exit_no_follow_up() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: "fatal stream error".to_string(),
            usage: None,
            error_kind: None,
        },
    ]]));

    let follow_up_polled = Arc::new(AtomicBool::new(false));
    let follow_up_polled_clone = Arc::clone(&follow_up_polled);

    let mut config = default_config(stream_fn);
    config.message_provider = Some(Arc::new(MockMessageProvider::follow_up_only(move || {
        follow_up_polled_clone.store(true, Ordering::SeqCst);
        vec![]
    })));

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    assert!(
        !follow_up_polled.load(Ordering::SeqCst),
        "follow-up should NOT be polled on error exit"
    );
}

// ─── 3.10: Abort via CancellationToken ───────────────────────────────────

#[tokio::test]
async fn abort() {
    let token = CancellationToken::new();
    let token_for_assertion = token.clone();
    let stream_fn = Arc::new(CancelsOnTextStartStreamFn);

    let config = default_config(stream_fn);

    let events = collect_events(agent_loop(vec![], "system".to_string(), config, token)).await;

    assert!(token_for_assertion.is_cancelled());
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::MessageEnd { message }
            if message.stop_reason == StopReason::Aborted
    )));
    assert!(has_event(&events, "AgentEnd"));
}

// ─── 3.11: Retry success ─────────────────────────────────────────────────

#[tokio::test]
async fn retry_success() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_message: "rate limit exceeded (429)".to_string(),
                usage: None,
                error_kind: None,
            },
        ],
        text_only_events("retried successfully"),
    ]));

    let mut config = default_config(stream_fn);
    config.retry_strategy = Box::new(
        DefaultRetryStrategy::default()
            .with_max_attempts(3)
            .with_jitter(false)
            .with_base_delay(Duration::from_millis(1)),
    );

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    let has_successful_end = events.iter().any(|e| {
        matches!(e, AgentEvent::MessageEnd { message } if message.stop_reason == StopReason::Stop)
    });
    assert!(
        has_successful_end,
        "should have a successful message after retry"
    );
}

#[tokio::test]
async fn retry_success_emits_one_logical_message_lifecycle() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "stale partial".to_string(),
            },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_message: "rate limit exceeded (429)".to_string(),
                usage: None,
                error_kind: None,
            },
        ],
        text_only_events("retried successfully"),
    ]));

    let mut config = default_config(stream_fn);
    config.retry_strategy = Box::new(
        DefaultRetryStrategy::default()
            .with_max_attempts(3)
            .with_jitter(false)
            .with_base_delay(Duration::from_millis(1)),
    );

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert_eq!(
        count_events(&events, "MessageStart"),
        1,
        "retry should preserve a single logical MessageStart"
    );
    assert_eq!(
        count_events(&events, "MessageEnd"),
        1,
        "retry should preserve a single logical MessageEnd"
    );

    let update_text = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageUpdate { delta } => Some(format!("{delta:?}")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !update_text.contains("stale partial"),
        "failed-attempt partials should not leak into the logical message lifecycle: {update_text}"
    );

    let final_text = events.iter().find_map(|event| match event {
        AgentEvent::MessageEnd { message } => Some(
            message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>(),
        ),
        _ => None,
    });

    assert_eq!(final_text.as_deref(), Some("retried successfully"));
}

// ─── 3.12: Non-retryable error ──────────────────────────────────────────

#[tokio::test]
async fn non_retryable_error() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_message: "fatal stream error".to_string(),
                usage: None,
                error_kind: None,
            },
        ],
        text_only_events("should not reach"),
    ]));

    let config = default_config(stream_fn);

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    let has_error_end = events.iter().any(|e| {
        matches!(e, AgentEvent::MessageEnd { message } if message.stop_reason == StopReason::Error)
    });
    assert!(has_error_end, "should have error MessageEnd");
}

// ─── 3.13: Max tokens recovery ──────────────────────────────────────────

#[tokio::test]
async fn max_tokens_recovery() {
    let events_with_incomplete = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_1".to_string(),
            name: "read_file".to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: r#"{"path": "/tmp"#.to_string(),
        },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Length,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ];

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        events_with_incomplete,
        text_only_events("recovered"),
    ]));

    let tool = Arc::new(MockTool::new("read_file"));
    let mut config = default_config(stream_fn);
    config.tools = vec![tool];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    assert_eq!(
        count_events(&events, "TurnStart"),
        2,
        "should have 2 turns — one with incomplete tool call, one with recovery"
    );
}

// Regression for #221: when the provider emits `ToolCallEnd` with truncated
// JSON alongside `StopReason::Length`, accumulation must preserve the
// incomplete block so the loop's recovery path converts it into an error
// tool result and continues.
#[tokio::test]
async fn max_tokens_recovery_with_tool_call_end() {
    let events_with_incomplete = vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: "tc_1".to_string(),
            name: "read_file".to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: r#"{"path": "/tmp"#.to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Length,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ];

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        events_with_incomplete,
        text_only_events("recovered"),
    ]));

    let tool = Arc::new(MockTool::new("read_file"));
    let mut config = default_config(stream_fn);
    config.tools = vec![tool];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    assert_eq!(
        count_events(&events, "TurnStart"),
        2,
        "should recover across two turns when ToolCallEnd carries truncated JSON"
    );
}

// ─── 3.14: convert_to_llm filter ────────────────────────────────────────

#[tokio::test]
async fn convert_to_llm_filter() {
    #[derive(Debug)]
    struct CustomMsg;
    impl CustomMessage for CustomMsg {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    let capturing_fn = Arc::new(MockContextCapturingStreamFn::new(vec![text_only_events(
        "ok",
    )]));

    let stream_fn: Arc<dyn StreamFn> = Arc::clone(&capturing_fn) as Arc<dyn StreamFn>;

    let mut config = default_config(stream_fn);
    config.convert_to_llm = Box::new(|msg: &AgentMessage| match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    });

    let messages = vec![
        AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        })),
        AgentMessage::Custom(Box::new(CustomMsg)),
    ];

    let events = collect_events(agent_loop(
        messages,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    let counts = capturing_fn.captured_message_counts.lock().unwrap();
    assert_eq!(
        counts[0], 1,
        "custom message should be filtered from provider input"
    );
    drop(counts);
}

// ─── 3.15: Overflow signal ───────────────────────────────────────────────

#[tokio::test]
async fn overflow_signal() {
    let overflow_flags: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let flags_clone = Arc::clone(&overflow_flags);

    let stream_fn = Arc::new(MockStreamFn::new(vec![
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::Error {
                stop_reason: StopReason::Error,
                error_message: "context window exceeded".to_string(),
                usage: None,
                error_kind: None,
            },
        ],
        text_only_events("recovered"),
    ]));

    let mut config = default_config(stream_fn);
    config.transform_context = Some(Arc::new(
        move |_msgs: &mut Vec<AgentMessage>, overflow: bool| {
            flags_clone.lock().unwrap().push(overflow);
        },
    ));

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    let flags = overflow_flags.lock().unwrap();
    assert!(
        flags.len() >= 2,
        "transform_context should be called at least twice"
    );
    assert!(!flags[0], "first call should not have overflow signal");
    assert!(flags[1], "second call should have overflow signal");
    drop(flags);
}

// ─── 3.16: No tool calls ─────────────────────────────────────────────────

#[tokio::test]
async fn no_tool_calls() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("Just text")]));
    let config = default_config(stream_fn);

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "TurnEnd"));
    assert!(!has_event(&events, "ToolExecutionStart"));
    assert!(!has_event(&events, "ToolExecutionEnd"));
}

// ─── 3.17: Validation failure ────────────────────────────────────────────

#[tokio::test]
async fn validation_failure() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "strict_tool", "{}"),
        text_only_events("after validation error"),
    ]));

    let tool = Arc::new(MockTool::new("strict_tool").with_schema(json!({
        "type": "object",
        "properties": {
            "path": { "type": "string" }
        },
        "required": ["path"],
        "additionalProperties": false
    })));
    let tool_clone = Arc::clone(&tool);

    let mut config = default_config(stream_fn);
    config.tools = vec![tool];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    let has_error_exec = events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolExecutionEnd { is_error, .. } if *is_error));
    assert!(has_error_exec, "should have error ToolExecutionEnd");
    assert!(
        !tool_clone.was_executed(),
        "execute should not be called when validation fails"
    );
}

// ─── MockPanickingTool ────────────────────────────────────────────────────

/// A tool that panics during execution.
struct MockPanickingTool {
    tool_name: String,
    panic_message: String,
}

impl MockPanickingTool {
    fn new(name: &str, panic_message: &str) -> Self {
        Self {
            tool_name: name.to_string(),
            panic_message: panic_message.to_string(),
        }
    }
}

impl AgentTool for MockPanickingTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn label(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &'static str {
        "A tool that panics for testing"
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
        Box::pin(async { panic!("{}", self.panic_message) })
    }
}

// ─── 3.18: Panicking tool produces error result ──────────────────────

#[tokio::test]
async fn panicking_tool_produces_error_result() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_panic", "panicking_tool", "{}"),
        text_only_events("after panic"),
    ]));

    let tool = Arc::new(MockPanickingTool::new(
        "panicking_tool",
        "deliberate test panic",
    ));
    let mut config = default_config(stream_fn);
    config.tools = vec![tool];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"), "loop should complete");

    let panic_starts = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::ToolExecutionStart { id, .. } if id == "tc_panic"
            )
        })
        .count();
    let panic_ends: Vec<&AgentToolResult> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionEnd {
                id,
                result,
                is_error,
                ..
            } if id == "tc_panic" && *is_error => Some(result),
            _ => None,
        })
        .collect();

    assert_eq!(
        panic_starts, 1,
        "panicking tool should still emit exactly one start event"
    );
    assert_eq!(
        panic_ends.len(),
        1,
        "panicking tool should emit a terminal error event"
    );
    assert!(
        ContentBlock::extract_text(&panic_ends[0].content).contains("deliberate test panic"),
        "terminal event should carry the panic payload"
    );

    // The panicked tool should produce a TurnEnd with an error tool result.
    let panic_tool_results: Vec<&ToolResultMessage> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnEnd { tool_results, .. } => Some(tool_results),
            _ => None,
        })
        .flatten()
        .filter(|r| r.tool_call_id == "tc_panic")
        .collect();

    assert!(
        !panic_tool_results.is_empty(),
        "panicked tool should produce a tool result, not be silently skipped"
    );

    let result = panic_tool_results[0];
    assert!(result.is_error, "panicked tool result should be an error");
    let text = ContentBlock::extract_text(&result.content);
    assert!(
        text.contains("tool execution panicked"),
        "error message should mention panic: {text}"
    );
    assert!(
        text.contains("deliberate test panic"),
        "error message should contain the panic payload: {text}"
    );
}

// ─── Turn snapshot tests ─────────────────────────────────────────────────

#[tokio::test]
async fn turn_end_carries_snapshot_with_messages() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("Hello!")]));
    let config = default_config(stream_fn);

    let events = collect_events(agent_loop(
        vec![common::user_msg("hi")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    let snapshot = events.iter().find_map(|e| match e {
        AgentEvent::TurnEnd { snapshot, .. } => Some(snapshot.clone()),
        _ => None,
    });

    let snapshot = snapshot.expect("TurnEnd should carry a snapshot");
    assert_eq!(snapshot.turn_index, 0);
    assert_eq!(snapshot.stop_reason, StopReason::Stop);
    // Should contain the user message + the assistant message
    assert!(
        snapshot.messages.len() >= 2,
        "snapshot should contain at least user + assistant messages, got {}",
        snapshot.messages.len()
    );
}

#[tokio::test]
async fn turn_snapshot_accumulates_across_tool_turns() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "my_tool", "{}"),
        text_only_events("Done!"),
    ]));

    let tool = Arc::new(MockTool::new("my_tool"));
    let mut config = default_config(stream_fn);
    config.tools = vec![tool];

    let events = collect_events(agent_loop(
        vec![common::user_msg("do something")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    let snapshots: Vec<TurnSnapshot> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnEnd { snapshot, .. } => Some(snapshot.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(snapshots.len(), 2, "should have two TurnEnd events");

    // First snapshot (tool turn): user + assistant
    assert_eq!(snapshots[0].turn_index, 0);
    assert_eq!(snapshots[0].stop_reason, StopReason::ToolUse);

    // Second snapshot (final turn): user + assistant + tool_result + assistant
    // turn_index is incremented after the first turn completes
    assert!(snapshots[1].turn_index >= snapshots[0].turn_index);
    assert_eq!(snapshots[1].stop_reason, StopReason::Stop);
    assert!(
        snapshots[1].messages.len() > snapshots[0].messages.len(),
        "second snapshot should have more messages than first"
    );
}

#[tokio::test]
async fn follow_up_turn_after_no_tool_turn_advances_turn_index() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first response"),
        text_only_events("second response"),
    ]));

    let follow_up_count = Arc::new(AtomicU32::new(0));
    let follow_up_clone = Arc::clone(&follow_up_count);

    let mut config = default_config(stream_fn);
    config.message_provider = Some(Arc::new(MockMessageProvider::follow_up_only(move || {
        let count = follow_up_clone.fetch_add(1, Ordering::SeqCst);
        if count == 0 {
            vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "follow up question".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            }))]
        } else {
            vec![]
        }
    })));

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    let snapshots: Vec<TurnSnapshot> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TurnEnd { snapshot, .. } => Some(snapshot.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(snapshots.len(), 2, "should have two completed turns");
    assert_eq!(snapshots[0].turn_index, 0);
    assert_eq!(
        snapshots[1].turn_index, 1,
        "the follow-up turn should observe the incremented turn index"
    );
}

#[tokio::test]
async fn steering_turn_after_no_tool_turn_continues_inner_loop() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("first response"),
        text_only_events("second response"),
    ]));

    let steering_count = Arc::new(AtomicU32::new(0));
    let steering_count_clone = Arc::clone(&steering_count);

    let mut config = default_config(stream_fn);
    config.message_provider = Some(Arc::new(MockMessageProvider::steering_only(move || {
        let count = steering_count_clone.fetch_add(1, Ordering::SeqCst);
        if count == 0 {
            vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "steering follow up".to_string(),
                }],
                timestamp: 0,
                cache_hint: None,
            }))]
        } else {
            vec![]
        }
    })));

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert_eq!(
        count_events(&events, "TurnStart"),
        2,
        "text-only turns should poll steering before breaking the inner loop"
    );

    let snapshots: Vec<TurnSnapshot> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TurnEnd { snapshot, .. } => Some(snapshot.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(snapshots.len(), 2, "steering should trigger a second turn");
    assert_eq!(snapshots[0].turn_index, 0);
    assert_eq!(snapshots[1].turn_index, 1);
}

#[tokio::test]
async fn pre_turn_new_messages_include_initial_prompt_batch() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("ok")]));
    let observations = Arc::new(Mutex::new(Vec::new()));

    let mut config = default_config(stream_fn);
    config.pre_turn_policies = vec![Arc::new(RecordingPreTurnPolicy {
        observations: Arc::clone(&observations),
    })];

    let events = collect_events(agent_loop(
        vec![common::user_msg("hello from prompt")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));
    let recorded = observations.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1, "pre-turn policy should run once");
    assert_eq!(
        recorded[0],
        RecordedPreTurnBatch {
            turn_index: 0,
            message_count: 1,
            new_messages: vec!["hello from prompt".to_string()],
        },
        "first-turn pre-turn policies must see the initial prompt batch as new_messages"
    );
}

#[tokio::test]
async fn post_turn_inject_without_tool_calls_continues_inner_loop() {
    let capturing_fn = Arc::new(MockContextCapturingStreamFn::new(vec![
        text_only_events("first response"),
        text_only_events("second response"),
    ]));
    let stream_fn: Arc<dyn StreamFn> = Arc::clone(&capturing_fn) as Arc<dyn StreamFn>;

    let mut config = default_config(stream_fn);
    config.post_turn_policies = vec![Arc::new(InjectingOncePostTurnPolicy {
        injected: AtomicBool::new(false),
        text: "policy follow-up".to_string(),
    })];

    let events = collect_events(agent_loop(
        vec![common::user_msg("start")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert_eq!(
        count_events(&events, "TurnStart"),
        2,
        "post-turn injections on text-only turns should schedule another inner-loop turn"
    );
    let counts = capturing_fn.captured_message_counts.lock().unwrap().clone();
    assert_eq!(
        counts,
        vec![1, 3],
        "second stream call should include the injected pending batch"
    );
}

#[tokio::test]
async fn turn_snapshot_serializes_to_json() {
    let snapshot = TurnSnapshot {
        turn_index: 3,
        messages: Arc::new(vec![]),
        usage: Usage {
            input: 100,
            output: 50,
            ..Default::default()
        },
        cost: Cost {
            total: 0.05,
            ..Default::default()
        },
        stop_reason: StopReason::Stop,
        state_delta: None,
    };

    let json = serde_json::to_string(&snapshot).expect("TurnSnapshot should serialize");
    let parsed: TurnSnapshot =
        serde_json::from_str(&json).expect("TurnSnapshot should deserialize");

    assert_eq!(parsed.turn_index, 3);
    assert_eq!(parsed.usage.input, 100);
    assert_eq!(parsed.usage.output, 50);
    assert!((parsed.cost.total - 0.05).abs() < f64::EPSILON);
    assert_eq!(parsed.stop_reason, StopReason::Stop);
    assert!(parsed.messages.is_empty());
}

// ─── Post-turn policy replaces assistant message before TurnEnd ──────────

/// A post-turn policy that replaces the assistant message text.
/// Simulates what `PiiRedactor` does: returns `Inject` with a modified
/// `AssistantMessage` to replace the original.
struct ReplacingPostTurnPolicy {
    replacement_text: String,
}

impl PostTurnPolicy for ReplacingPostTurnPolicy {
    fn name(&self) -> &'static str {
        "replacing-post-turn"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let orig = turn.assistant_message;
        let msg = AssistantMessage {
            content: vec![ContentBlock::Text {
                text: self.replacement_text.clone(),
            }],
            provider: orig.provider.clone(),
            model_id: orig.model_id.clone(),
            usage: orig.usage.clone(),
            cost: orig.cost.clone(),
            stop_reason: orig.stop_reason,
            error_message: orig.error_message.clone(),
            error_kind: orig.error_kind,
            timestamp: orig.timestamp,
            cache_hint: None,
        };
        PolicyVerdict::Inject(vec![AgentMessage::Llm(LlmMessage::Assistant(msg))])
    }
}

struct ToolInjectingPostTurnPolicy;

impl PostTurnPolicy for ToolInjectingPostTurnPolicy {
    fn name(&self) -> &'static str {
        "tool-injecting-post-turn"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let orig = turn.assistant_message;
        let msg = AssistantMessage {
            content: vec![ContentBlock::ToolCall {
                id: "call_injected".to_string(),
                name: "noop".to_string(),
                arguments: json!({}),
                partial_json: None,
            }],
            provider: orig.provider.clone(),
            model_id: orig.model_id.clone(),
            usage: orig.usage.clone(),
            cost: orig.cost.clone(),
            stop_reason: orig.stop_reason,
            error_message: orig.error_message.clone(),
            error_kind: orig.error_kind,
            timestamp: orig.timestamp,
            cache_hint: None,
        };
        PolicyVerdict::Inject(vec![AgentMessage::Llm(LlmMessage::Assistant(msg))])
    }
}

/// Regression test for #313: post-turn Inject verdicts must replace the
/// assistant message in `TurnEnd` and context BEFORE the event is emitted.
#[tokio::test]
async fn post_turn_inject_replaces_assistant_message_in_turn_end() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events(
        "secret SSN 123-45-6789",
    )]));

    let policy: Arc<dyn PostTurnPolicy> = Arc::new(ReplacingPostTurnPolicy {
        replacement_text: "secret SSN [REDACTED]".to_string(),
    });

    let mut config = default_config(stream_fn);
    config.post_turn_policies = vec![policy];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    // Find the TurnEnd event and verify the assistant message was replaced.
    let turn_end = events.iter().find_map(|e| match e {
        AgentEvent::TurnEnd {
            assistant_message, ..
        } => Some(assistant_message),
        _ => None,
    });

    let msg = turn_end.expect("should have TurnEnd event");
    let text = ContentBlock::extract_text(&msg.content);
    assert_eq!(
        text, "secret SSN [REDACTED]",
        "TurnEnd must contain the replaced assistant message, not the original"
    );

    // Verify the AgentEnd snapshot also contains the replaced message.
    let agent_end_messages = events.iter().find_map(|e| match e {
        AgentEvent::AgentEnd { messages } => Some(messages.clone()),
        _ => None,
    });
    let msgs = agent_end_messages.expect("should have AgentEnd");
    let last_assistant = msgs.iter().rev().find_map(|m| match m {
        AgentMessage::Llm(LlmMessage::Assistant(a)) => Some(a),
        _ => None,
    });
    let a = last_assistant.expect("should have assistant message in AgentEnd");
    let final_text = ContentBlock::extract_text(&a.content);
    assert_eq!(
        final_text, "secret SSN [REDACTED]",
        "AgentEnd context_messages must contain the replaced assistant message"
    );
}

#[tokio::test]
async fn post_turn_context_messages_include_committed_assistant_without_tools() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("Hello!")]));
    let observations = Arc::new(Mutex::new(Vec::new()));

    let mut config = default_config(stream_fn);
    config.post_turn_policies = vec![Arc::new(RecordingPostTurnPolicy {
        observations: Arc::clone(&observations),
    })];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "TurnEnd"));
    let recorded = observations.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1, "post-turn policy should run once");
    assert_eq!(
        recorded[0],
        RecordedTurnContext {
            message_count: 1,
            tool_result_count: 0,
            last_message_kind: "assistant",
        },
        "post-turn policies should observe the committed assistant snapshot even on text-only turns"
    );
}

#[tokio::test]
async fn post_turn_policies_run_on_terminal_error_stop() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![terminal_done_events(
        "fatal",
        StopReason::Error,
    )]));
    let observations = Arc::new(Mutex::new(Vec::new()));

    let mut config = default_config(stream_fn);
    config.post_turn_policies = vec![Arc::new(RecordingPostTurnPolicy {
        observations: Arc::clone(&observations),
    })];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TurnEnd {
            reason: swink_agent::TurnEndReason::Error,
            ..
        }
    )));
    let recorded = observations.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1, "post-turn policy should run once");
    assert_eq!(
        recorded[0],
        RecordedTurnContext {
            message_count: 1,
            tool_result_count: 0,
            last_message_kind: "assistant",
        },
        "terminal error turns should still expose the committed assistant snapshot to post-turn policies"
    );
}

#[tokio::test]
async fn post_turn_policies_run_on_terminal_aborted_stop() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![terminal_done_events(
        "partial",
        StopReason::Aborted,
    )]));
    let observations = Arc::new(Mutex::new(Vec::new()));

    let mut config = default_config(stream_fn);
    config.post_turn_policies = vec![Arc::new(RecordingPostTurnPolicy {
        observations: Arc::clone(&observations),
    })];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::TurnEnd {
            reason: swink_agent::TurnEndReason::Aborted,
            ..
        }
    )));
    let recorded = observations.lock().unwrap().clone();
    assert_eq!(recorded.len(), 1, "post-turn policy should run once");
    assert_eq!(
        recorded[0],
        RecordedTurnContext {
            message_count: 1,
            tool_result_count: 0,
            last_message_kind: "assistant",
        },
        "terminal aborted turns should still expose the committed assistant snapshot to post-turn policies"
    );
}

#[tokio::test]
async fn post_turn_inject_cannot_drop_tool_calls_from_turn_history() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "noop", "{}"),
        text_only_events("done"),
    ]));

    let policy: Arc<dyn PostTurnPolicy> = Arc::new(ReplacingPostTurnPolicy {
        replacement_text: "tool output [REDACTED]".to_string(),
    });

    let mut config = default_config(stream_fn);
    config.tools = vec![Arc::new(MockTool::new("noop"))];
    config.post_turn_policies = vec![policy];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    let turn_end_messages: Vec<&AssistantMessage> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TurnEnd {
                assistant_message, ..
            } => Some(assistant_message),
            _ => None,
        })
        .collect();
    let tool_turn_message = turn_end_messages
        .first()
        .expect("first turn should emit TurnEnd after tool execution");
    assert!(
        tool_turn_message.content.iter().any(
            |block| matches!(block, ContentBlock::ToolCall { id, name, arguments, .. }
                if id == "call_1" && name == "noop" && arguments == &json!({}))
        ),
        "tool-turn TurnEnd must keep the original tool call block",
    );
    assert_eq!(
        ContentBlock::extract_text(&tool_turn_message.content),
        "",
        "tool-turn replacement must not flatten tool calls into text"
    );

    let agent_end_messages = events.iter().find_map(|event| match event {
        AgentEvent::AgentEnd { messages } => Some(messages.clone()),
        _ => None,
    });
    let messages = agent_end_messages.expect("should have AgentEnd");
    let assistant_with_tool_call = messages
        .iter()
        .position(|message| match message {
            AgentMessage::Llm(LlmMessage::Assistant(assistant_message)) => assistant_message
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolCall { id, .. } if id == "call_1")),
            _ => false,
        })
        .expect("final history should keep the assistant tool call");
    assert!(
        matches!(
            messages.get(assistant_with_tool_call + 1),
            Some(AgentMessage::Llm(LlmMessage::ToolResult(result)))
                if result.tool_call_id == "call_1"
        ),
        "tool call must remain paired with its tool result in final history"
    );
}

#[tokio::test]
async fn post_turn_inject_cannot_add_tool_calls_to_text_only_turn() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("Hello!")]));
    let policy: Arc<dyn PostTurnPolicy> = Arc::new(ToolInjectingPostTurnPolicy);

    let mut config = default_config(stream_fn);
    config.post_turn_policies = vec![policy];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    let turn_end = events.iter().find_map(|event| match event {
        AgentEvent::TurnEnd {
            assistant_message, ..
        } => Some(assistant_message),
        _ => None,
    });
    let assistant_message = turn_end.expect("should emit TurnEnd");
    assert_eq!(
        ContentBlock::extract_text(&assistant_message.content),
        "Hello!",
        "text-only turn replacement must keep the original assistant text"
    );
    assert!(
        assistant_message
            .content
            .iter()
            .all(|block| !matches!(block, ContentBlock::ToolCall { .. })),
        "text-only turn replacement must not inject tool calls"
    );

    let agent_end_messages = events.iter().find_map(|event| match event {
        AgentEvent::AgentEnd { messages } => Some(messages.clone()),
        _ => None,
    });
    let messages = agent_end_messages.expect("should have AgentEnd");
    let last_assistant = messages.iter().rev().find_map(|message| match message {
        AgentMessage::Llm(LlmMessage::Assistant(assistant_message)) => Some(assistant_message),
        _ => None,
    });
    let final_assistant = last_assistant.expect("final history should contain assistant");
    assert!(
        final_assistant
            .content
            .iter()
            .all(|block| !matches!(block, ContentBlock::ToolCall { .. })),
        "final history must not contain injected tool calls"
    );
}

#[tokio::test]
async fn post_turn_policy_runs_before_transfer_termination() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_transfer", "handoff", "{}"),
        text_only_events("should not reach this"),
    ]));
    let observations = Arc::new(Mutex::new(Vec::new()));

    let mut config = default_config(stream_fn);
    config.tools = vec![Arc::new(MockTransferTool::new(
        "handoff",
        "billing",
        "billing question",
    ))];
    config.post_turn_policies = vec![Arc::new(RecordingPostTurnPolicy {
        observations: Arc::clone(&observations),
    })];

    let events = collect_events(agent_loop(
        vec![common::user_msg("transfer me")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    let recorded = observations.lock().unwrap().clone();
    assert_eq!(
        recorded.len(),
        1,
        "transfer turns must still run post-turn policies"
    );
    assert_eq!(
        recorded[0],
        RecordedTurnContext {
            message_count: 3,
            tool_result_count: 1,
            last_message_kind: "tool_result",
        },
        "transfer turns should expose the same committed turn snapshot shape as normal tool turns"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::TurnEnd {
                reason: swink_agent::TurnEndReason::Transfer,
                ..
            }
        )),
        "transfer turn should still terminate with TurnEndReason::Transfer"
    );
}

/// Regression test: post-turn Stop verdict still emits `TurnEnd` before stopping.
#[tokio::test]
async fn post_turn_stop_still_emits_turn_end() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("Hello!")]));
    let mut config = default_config(stream_fn);
    config.post_turn_policies = vec![Arc::new(StoppingPostTurnPolicy)];

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    // TurnEnd should still be emitted even when the policy stops the loop.
    assert!(has_event(&events, "TurnEnd"));
    assert!(has_event(&events, "AgentEnd"));
}

#[tokio::test]
async fn post_turn_stop_skips_follow_up_polling_without_tool_calls() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        text_only_events("Hello!"),
        text_only_events("unexpected follow-up"),
    ]));

    let follow_up_polled = Arc::new(AtomicBool::new(false));
    let follow_up_polled_clone = Arc::clone(&follow_up_polled);

    let mut config = default_config(stream_fn);
    config.post_turn_policies = vec![Arc::new(StoppingPostTurnPolicy)];
    config.message_provider = Some(Arc::new(MockMessageProvider::follow_up_only(move || {
        follow_up_polled_clone.store(true, Ordering::SeqCst);
        vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "follow up question".to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))]
    })));

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert_eq!(count_events(&events, "TurnStart"), 1);
    assert!(has_event(&events, "TurnEnd"));
    assert!(has_event(&events, "AgentEnd"));
    assert!(
        !follow_up_polled.load(Ordering::SeqCst),
        "follow-up should NOT be polled after a post-turn Stop"
    );
}

#[tokio::test]
async fn post_turn_stop_skips_follow_up_polling_after_tool_calls() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "noop", "{}"),
        text_only_events("unexpected follow-up"),
    ]));
    let tool = Arc::new(MockTool::new("noop"));

    let follow_up_polled = Arc::new(AtomicBool::new(false));
    let follow_up_polled_clone = Arc::clone(&follow_up_polled);

    let mut config = default_config(stream_fn);
    config.tools = vec![tool];
    config.post_turn_policies = vec![Arc::new(StoppingPostTurnPolicy)];
    config.message_provider = Some(Arc::new(MockMessageProvider::follow_up_only(move || {
        follow_up_polled_clone.store(true, Ordering::SeqCst);
        vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "follow up question".to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))]
    })));

    let events = collect_events(agent_loop(
        vec![],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert_eq!(count_events(&events, "TurnStart"), 1);
    assert!(has_event(&events, "TurnEnd"));
    assert!(has_event(&events, "AgentEnd"));
    assert!(
        !follow_up_polled.load(Ordering::SeqCst),
        "follow-up should NOT be polled after a post-turn Stop"
    );
}
