mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{MockStreamFn, MockTool, default_model, text_only_events, tool_call_events};
use futures::Stream;
use futures::stream::StreamExt;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AgentEvent, AgentLoopConfig, AgentMessage, AgentTool, AgentToolResult,
    AssistantMessageEvent, ContentBlock, Cost, CustomMessage, DefaultRetryStrategy, LlmMessage,
    MessageProvider, ModelSpec, StopReason, StreamFn, StreamOptions, ToolResultMessage,
    TurnSnapshot, Usage, UserMessage, agent_loop,
};

// ─── ContextCapturingStreamFn ────────────────────────────────────────────

/// A mock `StreamFn` that captures the messages passed in the context.
struct ContextCapturingStreamFn {
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    captured_message_counts: Mutex<Vec<usize>>,
}

impl ContextCapturingStreamFn {
    const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            captured_message_counts: Mutex::new(Vec::new()),
        }
    }
}

/// A mock `StreamFn` that captures resolved API keys from stream options.
struct ApiKeyCapturingStreamFn {
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    captured_api_keys: Mutex<Vec<Option<String>>>,
}

impl ApiKeyCapturingStreamFn {
    const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            captured_api_keys: Mutex::new(Vec::new()),
        }
    }
}

impl StreamFn for ApiKeyCapturingStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a AgentContext,
        options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.captured_api_keys
            .lock()
            .unwrap()
            .push(options.api_key.clone());
        let events = {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                vec![AssistantMessageEvent::Error {
                    stop_reason: StopReason::Error,
                    error_message: "no more scripted responses".to_string(),
                    usage: None,
                    error_kind: None,
                }]
            } else {
                responses.remove(0)
            }
        };
        Box::pin(futures::stream::iter(events))
    }
}

impl StreamFn for ContextCapturingStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.captured_message_counts
            .lock()
            .unwrap()
            .push(context.messages.len());
        let events = {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                vec![AssistantMessageEvent::Error {
                    stop_reason: StopReason::Error,
                    error_message: "no more scripted responses".to_string(),
                    usage: None,
                    error_kind: None,
                }]
            } else {
                responses.remove(0)
            }
        };
        Box::pin(futures::stream::iter(events))
    }
}

// ─── UpdatingTool ─────────────────────────────────────────────────────────

struct UpdatingTool {
    tool_name: String,
}

impl UpdatingTool {
    fn new(name: &str) -> Self {
        Self {
            tool_name: name.to_string(),
        }
    }
}

impl AgentTool for UpdatingTool {
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

// ─── Helper functions ────────────────────────────────────────────────────

/// Test helper that delegates to closures for steering/follow-up.
struct MockMessageProvider {
    steering: Box<dyn Fn() -> Vec<AgentMessage> + Send + Sync>,
    follow_up: Box<dyn Fn() -> Vec<AgentMessage> + Send + Sync>,
}

impl MockMessageProvider {
    fn steering_only(f: impl Fn() -> Vec<AgentMessage> + Send + Sync + 'static) -> Self {
        Self {
            steering: Box::new(f),
            follow_up: Box::new(Vec::new),
        }
    }

    fn follow_up_only(f: impl Fn() -> Vec<AgentMessage> + Send + Sync + 'static) -> Self {
        Self {
            steering: Box::new(Vec::new),
            follow_up: Box::new(f),
        }
    }
}

impl MessageProvider for MockMessageProvider {
    fn poll_steering(&self) -> Vec<AgentMessage> {
        (self.steering)()
    }

    fn poll_follow_up(&self) -> Vec<AgentMessage> {
        (self.follow_up)()
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
        approve_tool: None,
        approval_mode: swink_agent::ApprovalMode::default(),
        tool_validator: None,
        loop_policy: None,
        tool_call_transformer: None,
        post_turn_hook: None,
        async_transform_context: None,
        metrics_collector: None,
        fallback: None,
        budget_guard: None,
        tool_execution_policy: swink_agent::ToolExecutionPolicy::default(),
    }
}

/// Collect all events from a loop stream.
async fn collect_events(stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>) -> Vec<AgentEvent> {
    stream.collect().await
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

// ─── 3.1: Single-turn no-tool ────────────────────────────────────────────

#[tokio::test]
async fn test_3_1_single_turn_no_tool() {
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
async fn test_3_2_single_turn_with_tool_call() {
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
async fn test_3_3_multi_turn() {
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
async fn test_3_4_transform_context_ordering() {
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

// ─── 3.5: get_api_key ────────────────────────────────────────────────────

#[tokio::test]
async fn test_3_5_get_api_key() {
    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let calls_clone = Arc::clone(&calls);

    let stream_fn = Arc::new(ApiKeyCapturingStreamFn::new(vec![
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
async fn test_3_5b_tool_execution_update_events() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        tool_call_events("tc_1", "updating_tool", "{}"),
        text_only_events("done"),
    ]));

    let tool = Arc::new(UpdatingTool::new("updating_tool"));
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
            AgentEvent::ToolExecutionUpdate { partial } => {
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
async fn test_3_6_concurrent_execution() {
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
        elapsed < Duration::from_millis(250),
        "tools should execute concurrently, took {elapsed:?}"
    );
}

// ─── 3.7: Steering interrupt ─────────────────────────────────────────────

#[tokio::test]
async fn test_3_7_steering_interrupt() {
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

// ─── 3.8: Follow-up ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_3_8_follow_up() {
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
async fn test_3_9_error_exit_no_follow_up() {
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
async fn test_3_10_abort() {
    let token = CancellationToken::new();
    let token_clone = token.clone();

    let stream_fn = Arc::new(MockStreamFn::new(vec![{
        let mut events = vec![AssistantMessageEvent::Start];
        for i in 0..100 {
            events.push(AssistantMessageEvent::TextStart { content_index: i });
            events.push(AssistantMessageEvent::TextDelta {
                content_index: i,
                delta: "x".to_string(),
            });
            events.push(AssistantMessageEvent::TextEnd { content_index: i });
        }
        events.push(AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
        });
        events
    }]));

    let config = default_config(stream_fn);

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(5)).await;
        token_clone.cancel();
    });

    let events = collect_events(agent_loop(vec![], "system".to_string(), config, token)).await;

    assert!(has_event(&events, "AgentEnd"));
}

// ─── 3.11: Retry success ─────────────────────────────────────────────────

#[tokio::test]
async fn test_3_11_retry_success() {
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

// ─── 3.12: Non-retryable error ──────────────────────────────────────────

#[tokio::test]
async fn test_3_12_non_retryable_error() {
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
async fn test_3_13_max_tokens_recovery() {
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

// ─── 3.14: convert_to_llm filter ────────────────────────────────────────

#[tokio::test]
async fn test_3_14_convert_to_llm_filter() {
    #[derive(Debug)]
    struct CustomMsg;
    impl CustomMessage for CustomMsg {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    let capturing_fn = Arc::new(ContextCapturingStreamFn::new(vec![text_only_events("ok")]));

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
async fn test_3_15_overflow_signal() {
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
async fn test_3_16_no_tool_calls() {
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
async fn test_3_17_validation_failure() {
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

// ─── PanickingTool ────────────────────────────────────────────────────

/// A tool that panics during execution.
struct PanickingTool {
    tool_name: String,
    panic_message: String,
}

impl PanickingTool {
    fn new(name: &str, panic_message: &str) -> Self {
        Self {
            tool_name: name.to_string(),
            panic_message: panic_message.to_string(),
        }
    }
}

impl AgentTool for PanickingTool {
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

    let tool = Arc::new(PanickingTool::new(
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
