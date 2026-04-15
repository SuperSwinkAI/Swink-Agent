#![cfg(feature = "testkit")]
//! Tests for `ToolExecutionPolicy` — verifying sequential, priority, and
//! custom dispatch modes.

mod common;

use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{Stream, StreamExt};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentEvent, AgentLoopConfig, AgentMessage, AgentTool, AgentToolResult, AssistantMessageEvent,
    ContentBlock, Cost, DefaultRetryStrategy, LlmMessage, StopReason, StreamFn, StreamOptions,
    ToolCallSummary, ToolExecutionPolicy, ToolExecutionStrategy, Usage, UserMessage,
};

use common::{MockStreamFn, default_model, text_only_events};

// ─── OrderedMockTool ─────────────────────────────────────────────────────────

/// A mock tool that records its execution order via a shared counter.
struct OrderedMockTool {
    tool_name: String,
    schema: serde_json::Value,
    /// Shared counter incremented on each tool execution across all tools.
    order_counter: Arc<AtomicU32>,
    /// Records the order number when this tool executed.
    execution_order: Arc<Mutex<Vec<u32>>>,
    delay: Option<Duration>,
}

impl OrderedMockTool {
    fn new(
        name: &str,
        order_counter: Arc<AtomicU32>,
        execution_order: Arc<Mutex<Vec<u32>>>,
    ) -> Self {
        Self {
            tool_name: name.to_string(),
            schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": true
            }),
            order_counter,
            execution_order,
            delay: None,
        }
    }

    const fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }
}

impl AgentTool for OrderedMockTool {
    fn name(&self) -> &str {
        &self.tool_name
    }

    fn label(&self) -> &str {
        &self.tool_name
    }

    fn description(&self) -> &'static str {
        "ordered mock tool"
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
        _state: std::sync::Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::ResolvedCredential>,
    ) -> Pin<Box<dyn std::future::Future<Output = AgentToolResult> + Send + '_>> {
        let order = self.order_counter.fetch_add(1, Ordering::SeqCst);
        self.execution_order.lock().unwrap().push(order);
        let delay = self.delay;
        Box::pin(async move {
            if let Some(d) = delay {
                tokio::time::sleep(d).await;
            }
            AgentToolResult::text("ok")
        })
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

type ConvertToLlmBoxed = Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync>;

fn default_convert_to_llm() -> ConvertToLlmBoxed {
    Box::new(|msg| match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    })
}

fn make_config(
    stream_fn: Arc<dyn StreamFn>,
    tools: Vec<Arc<dyn AgentTool>>,
    policy: ToolExecutionPolicy,
) -> AgentLoopConfig {
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
        tools,
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
        tool_execution_policy: policy,
        session_state: std::sync::Arc::new(
            std::sync::RwLock::new(swink_agent::SessionState::new()),
        ),
        credential_resolver: None,
        cache_config: None,
        cache_state: std::sync::Mutex::new(swink_agent::CacheState::default()),
        dynamic_system_prompt: None,
        steering_interrupt: None,
    }
}

/// Build events that call three tools, then a text-only response.
fn three_tool_call_events() -> Vec<Vec<AssistantMessageEvent>> {
    vec![
        // Turn 1: call all three tools
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: "call_a".to_string(),
                name: "tool_a".to_string(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: "{}".to_string(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::ToolCallStart {
                content_index: 1,
                id: "call_b".to_string(),
                name: "tool_b".to_string(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 1,
                delta: "{}".to_string(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 1 },
            AssistantMessageEvent::ToolCallStart {
                content_index: 2,
                id: "call_c".to_string(),
                name: "tool_c".to_string(),
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
        ],
        // Turn 2: done
        text_only_events("done"),
    ]
}

async fn collect_events(stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>) -> Vec<AgentEvent> {
    stream.collect().await
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sequential_policy_executes_tools_in_order() {
    let order_counter = Arc::new(AtomicU32::new(0));
    let order_a = Arc::new(Mutex::new(Vec::new()));
    let order_b = Arc::new(Mutex::new(Vec::new()));
    let order_c = Arc::new(Mutex::new(Vec::new()));

    let tool_a = Arc::new(OrderedMockTool::new(
        "tool_a",
        Arc::clone(&order_counter),
        Arc::clone(&order_a),
    )) as Arc<dyn AgentTool>;
    let tool_b = Arc::new(OrderedMockTool::new(
        "tool_b",
        Arc::clone(&order_counter),
        Arc::clone(&order_b),
    )) as Arc<dyn AgentTool>;
    let tool_c = Arc::new(OrderedMockTool::new(
        "tool_c",
        Arc::clone(&order_counter),
        Arc::clone(&order_c),
    )) as Arc<dyn AgentTool>;

    let stream_fn = Arc::new(MockStreamFn::new(three_tool_call_events()));
    let config = make_config(
        stream_fn,
        vec![tool_a, tool_b, tool_c],
        ToolExecutionPolicy::Sequential,
    );

    let prompt = vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: "go".to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))];

    let stream =
        swink_agent::agent_loop(prompt, "test".to_string(), config, CancellationToken::new());
    let _ = collect_events(stream).await;

    // In sequential mode, tool_a should execute first (order 0), then
    // tool_b (order 1), then tool_c (order 2).
    let a = order_a.lock().unwrap()[0];
    let b = order_b.lock().unwrap()[0];
    let c = order_c.lock().unwrap()[0];

    assert_eq!(a, 0, "tool_a should execute first");
    assert_eq!(b, 1, "tool_b should execute second");
    assert_eq!(c, 2, "tool_c should execute third");
}

#[tokio::test]
async fn priority_policy_executes_higher_priority_first() {
    let order_counter = Arc::new(AtomicU32::new(0));
    let order_a = Arc::new(Mutex::new(Vec::new()));
    let order_b = Arc::new(Mutex::new(Vec::new()));
    let order_c = Arc::new(Mutex::new(Vec::new()));

    // Each tool gets a small delay to prevent timing races in sequential groups
    let tool_a = Arc::new(
        OrderedMockTool::new("tool_a", Arc::clone(&order_counter), Arc::clone(&order_a))
            .with_delay(Duration::from_millis(5)),
    ) as Arc<dyn AgentTool>;
    let tool_b = Arc::new(
        OrderedMockTool::new("tool_b", Arc::clone(&order_counter), Arc::clone(&order_b))
            .with_delay(Duration::from_millis(5)),
    ) as Arc<dyn AgentTool>;
    let tool_c = Arc::new(
        OrderedMockTool::new("tool_c", Arc::clone(&order_counter), Arc::clone(&order_c))
            .with_delay(Duration::from_millis(5)),
    ) as Arc<dyn AgentTool>;

    let stream_fn = Arc::new(MockStreamFn::new(three_tool_call_events()));

    // Priority: tool_c=10, tool_a=5, tool_b=1
    // Each in its own priority group → all sequential.
    let priority_fn: Arc<swink_agent::PriorityFn> =
        Arc::new(|summary: &ToolCallSummary<'_>| match summary.name {
            "tool_c" => 10,
            "tool_a" => 5,
            "tool_b" => 1,
            _ => 0,
        });

    let config = make_config(
        stream_fn,
        vec![tool_a, tool_b, tool_c],
        ToolExecutionPolicy::Priority(priority_fn),
    );

    let prompt = vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: "go".to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))];

    let stream =
        swink_agent::agent_loop(prompt, "test".to_string(), config, CancellationToken::new());
    let _ = collect_events(stream).await;

    let a = order_a.lock().unwrap()[0];
    let b = order_b.lock().unwrap()[0];
    let c = order_c.lock().unwrap()[0];

    // tool_c (priority 10) should go first, then tool_a (5), then tool_b (1)
    assert!(
        c < a,
        "tool_c (pri=10) should run before tool_a (pri=5): c={c}, a={a}"
    );
    assert!(
        a < b,
        "tool_a (pri=5) should run before tool_b (pri=1): a={a}, b={b}"
    );
}

#[tokio::test]
async fn concurrent_policy_is_default_and_spawns_all() {
    let order_counter = Arc::new(AtomicU32::new(0));
    let order_a = Arc::new(Mutex::new(Vec::new()));
    let order_b = Arc::new(Mutex::new(Vec::new()));

    // Give tool_a a delay — in concurrent mode, tool_b should still start
    // (potentially before tool_a finishes).
    let tool_a = Arc::new(
        OrderedMockTool::new("tool_a", Arc::clone(&order_counter), Arc::clone(&order_a))
            .with_delay(Duration::from_millis(50)),
    ) as Arc<dyn AgentTool>;
    let tool_b = Arc::new(OrderedMockTool::new(
        "tool_b",
        Arc::clone(&order_counter),
        Arc::clone(&order_b),
    )) as Arc<dyn AgentTool>;

    let events = vec![
        vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::ToolCallStart {
                content_index: 0,
                id: "call_a".to_string(),
                name: "tool_a".to_string(),
            },
            AssistantMessageEvent::ToolCallDelta {
                content_index: 0,
                delta: "{}".to_string(),
            },
            AssistantMessageEvent::ToolCallEnd { content_index: 0 },
            AssistantMessageEvent::ToolCallStart {
                content_index: 1,
                id: "call_b".to_string(),
                name: "tool_b".to_string(),
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
        text_only_events("done"),
    ];

    let stream_fn = Arc::new(MockStreamFn::new(events));
    let config = make_config(
        stream_fn,
        vec![tool_a, tool_b],
        ToolExecutionPolicy::Concurrent,
    );

    let prompt = vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: "go".to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))];

    let stream =
        swink_agent::agent_loop(prompt, "test".to_string(), config, CancellationToken::new());
    let _ = collect_events(stream).await;

    // Both tools should have executed.
    assert!(
        !order_a.lock().unwrap().is_empty(),
        "tool_a should have executed"
    );
    assert!(
        !order_b.lock().unwrap().is_empty(),
        "tool_b should have executed"
    );
}

#[tokio::test]
async fn custom_strategy_controls_grouping() {
    /// A custom strategy that puts each tool in its own group (i.e. sequential)
    /// but in reverse order.
    struct ReverseSequentialStrategy;

    impl ToolExecutionStrategy for ReverseSequentialStrategy {
        fn partition(
            &self,
            tool_calls: &[ToolCallSummary<'_>],
        ) -> Pin<Box<dyn std::future::Future<Output = Vec<Vec<usize>>> + Send + '_>> {
            let count = tool_calls.len();
            Box::pin(async move { (0..count).rev().map(|i| vec![i]).collect() })
        }
    }

    let order_counter = Arc::new(AtomicU32::new(0));
    let order_a = Arc::new(Mutex::new(Vec::new()));
    let order_b = Arc::new(Mutex::new(Vec::new()));
    let order_c = Arc::new(Mutex::new(Vec::new()));

    let tool_a = Arc::new(OrderedMockTool::new(
        "tool_a",
        Arc::clone(&order_counter),
        Arc::clone(&order_a),
    )) as Arc<dyn AgentTool>;
    let tool_b = Arc::new(OrderedMockTool::new(
        "tool_b",
        Arc::clone(&order_counter),
        Arc::clone(&order_b),
    )) as Arc<dyn AgentTool>;
    let tool_c = Arc::new(OrderedMockTool::new(
        "tool_c",
        Arc::clone(&order_counter),
        Arc::clone(&order_c),
    )) as Arc<dyn AgentTool>;

    let stream_fn = Arc::new(MockStreamFn::new(three_tool_call_events()));
    let config = make_config(
        stream_fn,
        vec![tool_a, tool_b, tool_c],
        ToolExecutionPolicy::Custom(Arc::new(ReverseSequentialStrategy)),
    );

    let prompt = vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: "go".to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))];

    let stream =
        swink_agent::agent_loop(prompt, "test".to_string(), config, CancellationToken::new());
    let _ = collect_events(stream).await;

    let a = order_a.lock().unwrap()[0];
    let b = order_b.lock().unwrap()[0];
    let c = order_c.lock().unwrap()[0];

    // Reverse order: tool_c first (idx 2 → group 0), tool_b second, tool_a last.
    assert_eq!(c, 0, "tool_c should execute first (reversed)");
    assert_eq!(b, 1, "tool_b should execute second (reversed)");
    assert_eq!(a, 2, "tool_a should execute third (reversed)");
}

#[tokio::test]
async fn priority_groups_with_equal_priority_run_concurrently() {
    let order_counter = Arc::new(AtomicU32::new(0));
    let order_a = Arc::new(Mutex::new(Vec::new()));
    let order_b = Arc::new(Mutex::new(Vec::new()));
    let order_c = Arc::new(Mutex::new(Vec::new()));

    // Give all tools a delay so concurrent tools overlap in time.
    let tool_a = Arc::new(
        OrderedMockTool::new("tool_a", Arc::clone(&order_counter), Arc::clone(&order_a))
            .with_delay(Duration::from_millis(10)),
    ) as Arc<dyn AgentTool>;
    let tool_b = Arc::new(
        OrderedMockTool::new("tool_b", Arc::clone(&order_counter), Arc::clone(&order_b))
            .with_delay(Duration::from_millis(10)),
    ) as Arc<dyn AgentTool>;
    let tool_c = Arc::new(
        OrderedMockTool::new("tool_c", Arc::clone(&order_counter), Arc::clone(&order_c))
            .with_delay(Duration::from_millis(10)),
    ) as Arc<dyn AgentTool>;

    let stream_fn = Arc::new(MockStreamFn::new(three_tool_call_events()));

    // tool_a and tool_b share priority 10 (same group, concurrent).
    // tool_c has priority 1 (lower, sequential after the first group).
    let priority_fn: Arc<swink_agent::PriorityFn> =
        Arc::new(|summary: &ToolCallSummary<'_>| match summary.name {
            "tool_a" | "tool_b" => 10,
            "tool_c" => 1,
            _ => 0,
        });

    let config = make_config(
        stream_fn,
        vec![tool_a, tool_b, tool_c],
        ToolExecutionPolicy::Priority(priority_fn),
    );

    let prompt = vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: "go".to_string(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))];

    let stream =
        swink_agent::agent_loop(prompt, "test".to_string(), config, CancellationToken::new());
    let _ = collect_events(stream).await;

    let a = order_a.lock().unwrap()[0];
    let b = order_b.lock().unwrap()[0];
    let c = order_c.lock().unwrap()[0];

    // tool_a and tool_b (priority 10) should both run before tool_c (priority 1).
    assert!(
        a < c,
        "tool_a (pri=10) should run before tool_c (pri=1): a={a}, c={c}"
    );
    assert!(
        b < c,
        "tool_b (pri=10) should run before tool_c (pri=1): b={b}, c={c}"
    );
}
