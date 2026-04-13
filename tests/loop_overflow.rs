#![cfg(feature = "testkit")]
//! Integration tests for emergency in-place overflow recovery (US6, T064-T073).
//!
//! Tests verify that context overflow triggers emergency recovery within the
//! same turn, not across turns.

mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    MockContextCapturingStreamFn, MockStreamFn, MockTool, default_model, next_response,
    text_only_events, tool_call_events, user_msg,
};
use futures::Stream;
use futures::stream::StreamExt;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AgentEvent, AgentLoopConfig, AgentMessage, AssistantMessageEvent, ContentBlock,
    DefaultRetryStrategy, LlmMessage, ModelSpec, StreamFn, StreamOptions, UserMessage, agent_loop,
};

// ─── Helpers ────────────────────────────────────────────────────────────────

fn overflow_error_events() -> Vec<AssistantMessageEvent> {
    vec![AssistantMessageEvent::error_context_overflow(
        "context_length_exceeded: too many tokens",
    )]
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
        pre_turn_policies: vec![],
        pre_dispatch_policies: vec![],
        post_turn_policies: vec![],
        post_loop_policies: vec![],
        async_transform_context: None,
        metrics_collector: None,
        fallback: None,
        tool_execution_policy: swink_agent::ToolExecutionPolicy::default(),
        session_state: Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
        credential_resolver: None,
        cache_config: None,
        cache_state: std::sync::Mutex::new(swink_agent::CacheState::default()),
        dynamic_system_prompt: None,
    }
}

fn large_user_msg(label: &str, token_count: usize) -> AgentMessage {
    let padding = "x".repeat(token_count * 4);
    let text = format!("{label}:{padding}");
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text { text }],
        timestamp: 0,
        cache_hint: None,
    }))
}

async fn collect_events(stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>) -> Vec<AgentEvent> {
    stream.collect().await
}

fn has_event(events: &[AgentEvent], name: &str) -> bool {
    events.iter().any(|e| common::event_variant_name(e) == name)
}

fn count_events(events: &[AgentEvent], name: &str) -> usize {
    events
        .iter()
        .filter(|e| common::event_variant_name(e) == name)
        .count()
}

// ─── Mock async context transformer ─────────────────────────────────────────

/// An async context transformer that tracks calls and whether overflow was set.
struct MockAsyncTransformer {
    overflow_flags: Arc<Mutex<Vec<bool>>>,
    compact: bool,
}

impl swink_agent::AsyncContextTransformer for MockAsyncTransformer {
    fn transform<'a>(
        &'a self,
        messages: &'a mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Pin<Box<dyn Future<Output = Option<swink_agent::CompactionReport>> + Send + 'a>> {
        self.overflow_flags.lock().unwrap().push(overflow);
        let compact = self.compact;
        Box::pin(async move {
            if overflow && compact && messages.len() > 1 {
                let removed = messages.len() - 1;
                messages.truncate(1);
                Some(swink_agent::CompactionReport {
                    dropped_count: removed,
                    tokens_before: removed * 100,
                    tokens_after: 100,
                    overflow: true,
                    dropped_messages: Vec::new(),
                })
            } else {
                None
            }
        })
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// T064: Emergency overflow recovery — overflow on first call, success on retry
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn emergency_overflow_recovery() {
    // Call 1: overflow. In-place recovery compacts. Call 2 (retry): success.
    let capturing_fn = Arc::new(MockContextCapturingStreamFn::new(vec![
        overflow_error_events(),
        text_only_events("recovered after compaction"),
    ]));
    let stream_fn: Arc<dyn StreamFn> = Arc::clone(&capturing_fn) as Arc<dyn StreamFn>;

    // Track overflow flags for both transformers.
    let async_flags: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let sync_flags: Arc<Mutex<Vec<bool>>> = Arc::new(Mutex::new(Vec::new()));
    let sync_flags_clone = Arc::clone(&sync_flags);

    let mut config = default_config(stream_fn);

    // Async transformer: compacts on overflow
    config.async_transform_context = Some(Arc::new(MockAsyncTransformer {
        overflow_flags: Arc::clone(&async_flags),
        compact: true,
    }));

    // Sync transformer: tracks calls but does not compact
    config.transform_context = Some(Arc::new(
        move |_msgs: &mut Vec<AgentMessage>, overflow: bool| {
            sync_flags_clone.lock().unwrap().push(overflow);
        },
    ));

    let mut initial_messages = Vec::new();
    for i in 0..5 {
        initial_messages.push(large_user_msg(&format!("msg{i}"), 100));
    }

    let events = collect_events(agent_loop(
        initial_messages,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"), "loop should complete");

    // Verify async transformer was called with overflow=true during recovery.
    let af = async_flags.lock().unwrap().clone();
    assert!(af.len() >= 2, "async transformer called at least twice");
    assert!(!af[0], "first call: overflow=false (pre-turn)");
    assert!(af[1], "second call: overflow=true (recovery)");

    // Verify sync transformer was called with overflow=true during recovery.
    let sf = sync_flags.lock().unwrap().clone();
    assert!(sf.len() >= 2, "sync transformer called at least twice");
    assert!(!sf[0], "first call: overflow=false (pre-turn)");
    assert!(sf[1], "second call: overflow=true (recovery)");

    // Verify ContextCompacted event was emitted.
    assert!(
        count_events(&events, "ContextCompacted") >= 1,
        "should emit at least one ContextCompacted event"
    );

    // Verify retry used compacted context.
    let counts = capturing_fn.captured_message_counts.lock().unwrap().clone();
    assert_eq!(counts.len(), 2, "exactly 2 stream calls");
    assert!(
        counts[1] < counts[0],
        "retry should see fewer messages: first={}, second={}",
        counts[0],
        counts[1]
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// T065: Double overflow surfaces error (no infinite loop)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn double_overflow_surfaces_error() {
    // Call 1: overflow. Recovery compacts. Call 2 (retry): overflow again.
    // Expected: error surfaced, no infinite loop.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        overflow_error_events(),
        overflow_error_events(),
    ]));

    let mut config = default_config(stream_fn as Arc<dyn StreamFn>);

    // Compacting transformer that always removes some messages.
    config.transform_context = Some(Arc::new(|msgs: &mut Vec<AgentMessage>, overflow: bool| {
        if overflow && msgs.len() > 1 {
            msgs.truncate(1);
        }
    }));

    let mut initial_messages = Vec::new();
    for i in 0..5 {
        initial_messages.push(user_msg(&format!("msg{i}")));
    }

    let events = collect_events(agent_loop(
        initial_messages,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    // Loop terminates.
    assert!(has_event(&events, "AgentEnd"));

    // No TurnEnd with ToolsExecuted — the turn ended due to error.
    assert!(
        events.iter().any(|e| {
            matches!(
                e,
                AgentEvent::TurnEnd { reason, .. }
                    if *reason == swink_agent::TurnEndReason::Error
            )
        }),
        "should have a TurnEnd with Error reason"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// T066: No transformer configured — overflow surfaces error immediately
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn no_transformer_overflow_surfaces_error() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        overflow_error_events(),
        text_only_events("should not reach this"),
    ]));

    // No transformers configured — neither async nor sync.
    let config = default_config(stream_fn as Arc<dyn StreamFn>);

    let events = collect_events(agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));

    // Should NOT have ContextCompacted (no transformer to run).
    assert_eq!(
        count_events(&events, "ContextCompacted"),
        0,
        "no ContextCompacted when no transformer"
    );

    // Should end with error.
    assert!(
        events.iter().any(|e| {
            matches!(
                e,
                AgentEvent::TurnEnd { reason, .. }
                    if *reason == swink_agent::TurnEndReason::Error
            )
        }),
        "should have TurnEnd with Error reason"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// T067: overflow_recovery_attempted resets at turn start — independent recovery
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn overflow_recovery_resets_per_turn() {
    // Turn 1: tool call (no overflow). Turn 2: overflow → recovery → success.
    // This verifies that overflow_recovery_attempted resets between turns.
    let stream_fn = Arc::new(MockStreamFn::new(vec![
        // Turn 1: tool call
        tool_call_events("tc_1", "mock_tool", "{}"),
        // Turn 2: overflow
        overflow_error_events(),
        // Turn 2 retry: success
        text_only_events("recovered in turn 2"),
    ]));

    let tool = Arc::new(MockTool::new("mock_tool"));

    let mut config = default_config(stream_fn as Arc<dyn StreamFn>);
    config.tools = vec![tool];
    config.transform_context = Some(Arc::new(|msgs: &mut Vec<AgentMessage>, overflow: bool| {
        if overflow && msgs.len() > 1 {
            msgs.truncate(1);
        }
    }));

    let mut initial_messages = Vec::new();
    for i in 0..5 {
        initial_messages.push(user_msg(&format!("msg{i}")));
    }

    let events = collect_events(agent_loop(
        initial_messages,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"), "loop should complete");

    // Should have at least 2 TurnStart events (turn 1 and turn 2).
    assert!(
        count_events(&events, "TurnStart") >= 2,
        "should have at least 2 turns"
    );

    // Should have ContextCompacted from the recovery in turn 2.
    assert!(
        count_events(&events, "ContextCompacted") >= 1,
        "turn 2 should recover from overflow"
    );

    // Should NOT end with an error — the recovery in turn 2 should succeed.
    assert!(
        !events.iter().any(|e| {
            matches!(
                e,
                AgentEvent::TurnEnd { reason, .. }
                    if *reason == swink_agent::TurnEndReason::Error
            )
        }),
        "should not have error TurnEnd — recovery should succeed"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// T072: No compaction skip (FR-013d) — transformers run but both return None
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn no_compaction_skip_surfaces_error() {
    // Transformers are configured but return None (no compaction occurred).
    // Error should be surfaced immediately without retrying the LLM call.
    let capturing_fn = Arc::new(MockContextCapturingStreamFn::new(vec![
        overflow_error_events(),
        text_only_events("should not reach this"),
    ]));
    let stream_fn: Arc<dyn StreamFn> = Arc::clone(&capturing_fn) as Arc<dyn StreamFn>;

    let mut config = default_config(stream_fn);

    // Async transformer: returns None (no-op).
    config.async_transform_context = Some(Arc::new(MockAsyncTransformer {
        overflow_flags: Arc::new(Mutex::new(Vec::new())),
        compact: false, // never compacts
    }));

    // Sync transformer: also returns None (no-op).
    config.transform_context = Some(Arc::new(
        |_msgs: &mut Vec<AgentMessage>, _overflow: bool| {
            // intentionally no-op — does not modify messages
        },
    ));

    let events = collect_events(agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"));

    // Only 1 stream call — no retry when transformers report no compaction.
    let counts = capturing_fn.captured_message_counts.lock().unwrap().clone();
    assert_eq!(
        counts.len(),
        1,
        "should have exactly 1 stream call (no retry when no compaction), got {}",
        counts.len()
    );

    // Should end with error.
    assert!(
        events.iter().any(|e| {
            matches!(
                e,
                AgentEvent::TurnEnd { reason, .. }
                    if *reason == swink_agent::TurnEndReason::Error
            )
        }),
        "should have TurnEnd with Error reason"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// T073: Cancellation during emergency recovery aborts the loop
// ═══════════════════════════════════════════════════════════════════════════════

/// A `StreamFn` that cancels the token after the first overflow, before the retry.
struct CancellingAfterOverflowStreamFn {
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    cancel_token: CancellationToken,
    call_count: std::sync::atomic::AtomicU32,
}

impl StreamFn for CancellingAfterOverflowStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let count = self
            .call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if count == 0 {
            // First call: return overflow and cancel the token so recovery
            // detects cancellation before retry.
            self.cancel_token.cancel();
        }
        let fallback = vec![AssistantMessageEvent::error("exhausted")];
        let events = next_response(&self.responses, fallback);
        Box::pin(futures::stream::iter(events))
    }
}

#[tokio::test]
async fn cancellation_during_recovery_aborts() {
    let cancel_token = CancellationToken::new();

    let stream_fn = Arc::new(CancellingAfterOverflowStreamFn {
        responses: Mutex::new(vec![
            overflow_error_events(),
            text_only_events("should not reach — cancelled"),
        ]),
        cancel_token: cancel_token.clone(),
        call_count: std::sync::atomic::AtomicU32::new(0),
    });

    let mut config = default_config(stream_fn as Arc<dyn StreamFn>);

    // Compacting transformer so recovery tries.
    config.transform_context = Some(Arc::new(|msgs: &mut Vec<AgentMessage>, overflow: bool| {
        if overflow && msgs.len() > 1 {
            msgs.truncate(1);
        }
    }));

    let events = collect_events(agent_loop(
        vec![user_msg("msg1"), user_msg("msg2"), user_msg("msg3")],
        "system".to_string(),
        config,
        cancel_token,
    ))
    .await;

    assert!(has_event(&events, "AgentEnd"), "loop should complete");

    // Should have a TurnEnd with Cancelled or Aborted reason.
    assert!(
        events.iter().any(|e| {
            matches!(
                e,
                AgentEvent::TurnEnd { reason, .. }
                    if matches!(reason, swink_agent::TurnEndReason::Cancelled | swink_agent::TurnEndReason::Aborted)
            )
        }),
        "should have TurnEnd with Cancelled/Aborted reason"
    );
}
