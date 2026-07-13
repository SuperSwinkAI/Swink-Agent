#![cfg(feature = "testkit")]
//! Regression tests for issue #771: cache-prefix tracking across turns.
//!
//! Before the fix, `CacheState::cached_prefix_len` stayed at 0 for the entire
//! agent loop — cache hinting and cache-protected compaction therefore never
//! activated in production. These tests drive a multi-turn flow with caching
//! enabled and assert:
//!   1. `cached_prefix_len` becomes non-zero after the first cache-eligible turn
//!      (observed through the `CacheAction { prefix_tokens }` event and through
//!      the `SlidingWindowTransformer` receiving a non-zero prefix on turn 2).
//!   2. Compaction respects the recorded prefix (leading anchor messages
//!      survive even when the transformer's own `anchor` would otherwise drop
//!      them).

mod common;

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    default_exhausted_fallback, default_model, error_events, next_response, text_only_events,
    user_msg,
};
use futures::Stream;
use futures::stream::StreamExt;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AgentError, AgentEvent, AgentLoopConfig, AgentMessage, AssistantMessageEvent,
    CacheConfig, CacheState, ContentBlock, DefaultRetryStrategy, LlmMessage, MessageProvider,
    ModelSpec, RetryStrategy, SlidingWindowTransformer, StopReason, StreamFn, StreamOptions,
    UserMessage, agent_loop,
};

// ─── Capturing stream that records the hint carried by each leading message ──

/// A scripted `StreamFn` that records, for each invocation, the `cache_hint`
/// present on each message in the provider-visible context.
struct CaptureHintsStreamFn {
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    per_call_hints: Arc<Mutex<Vec<Vec<Option<swink_agent::CacheHint>>>>>,
}

impl StreamFn for CaptureHintsStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let hints: Vec<Option<swink_agent::CacheHint>> = context
            .messages
            .iter()
            .map(|m| m.cache_hint().cloned())
            .collect();
        self.per_call_hints.lock().unwrap().push(hints);

        let events = next_response(&self.responses, default_exhausted_fallback());
        Box::pin(futures::stream::iter(events))
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

type ConvertToLlmBoxed = Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync>;

fn default_convert_to_llm() -> ConvertToLlmBoxed {
    Box::new(|msg: &AgentMessage| match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    })
}

fn base_config(stream_fn: Arc<dyn StreamFn>, cache_config: Option<CacheConfig>) -> AgentLoopConfig {
    let mut config = AgentLoopConfig::new(default_model(), stream_fn, default_convert_to_llm());
    config.retry_strategy = Box::new(
        DefaultRetryStrategy::default()
            .with_jitter(false)
            .with_base_delay(Duration::from_millis(1)),
    );
    config.cache_config = cache_config;
    config.cache_state = std::sync::Mutex::new(CacheState::default());
    config
}

struct RetryCacheMissOnce;

impl RetryStrategy for RetryCacheMissOnce {
    fn should_retry(&self, error: &AgentError, attempt: u32) -> bool {
        matches!(error, AgentError::CacheMiss) && attempt < 2
    }

    fn delay(&self, _attempt: u32) -> Duration {
        Duration::from_millis(0)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Create a user message whose text is roughly `token_count * 4` characters
/// (the `chars/4` default heuristic → `token_count` estimated tokens).
fn sized_user_msg(label: &str, token_count: usize) -> AgentMessage {
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

fn cache_actions(events: &[AgentEvent]) -> Vec<(swink_agent::CacheHint, usize)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::CacheAction {
                hint,
                prefix_tokens,
            } => Some((hint.clone(), *prefix_tokens)),
            _ => None,
        })
        .collect()
}

/// A `MessageProvider` that yields a single follow-up message on the first
/// `poll_follow_up` call, letting us drive a second turn without any tool
/// activity. All subsequent calls return empty.
struct OneShotFollowUp {
    msg: Mutex<Option<AgentMessage>>,
}

impl MessageProvider for OneShotFollowUp {
    fn poll_steering(&self) -> Vec<AgentMessage> {
        Vec::new()
    }
    fn poll_follow_up(&self) -> Vec<AgentMessage> {
        self.msg.lock().unwrap().take().into_iter().collect()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Regression test: prefix becomes non-zero after first cache-eligible turn.
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cached_prefix_len_tracked_after_first_turn() {
    let stream_fn = Arc::new(CaptureHintsStreamFn {
        responses: Mutex::new(vec![text_only_events("first"), text_only_events("second")]),
        per_call_hints: Arc::new(Mutex::new(Vec::new())),
    });
    let captured = Arc::clone(&stream_fn.per_call_hints);

    // Low min_tokens (10) so even a small prompt is cache-eligible on turn 1.
    // cache_intervals=3 → turn 1 Write, turns 2-3 Read, turn 4 Write-refresh.
    let cache_config = CacheConfig::new(Duration::from_mins(5), 10, 3);

    // Use `SlidingWindowTransformer` directly so the turn pipeline's downcast
    // hook can publish the cached prefix into it each turn.
    let transformer = Arc::new(SlidingWindowTransformer::new(10_000, 5_000, 1));

    let mut config = base_config(stream_fn.clone() as Arc<dyn StreamFn>, Some(cache_config));
    config.transform_context = Some(transformer.clone());
    config.message_provider = Some(Arc::new(OneShotFollowUp {
        msg: Mutex::new(Some(user_msg("follow-up question"))),
    }));

    // Initial prompt: ~50 tokens → well above `min_tokens=10`.
    let initial_messages = vec![sized_user_msg("prompt", 50)];

    let events = collect_events(agent_loop(
        initial_messages,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    // ── Stream-visible state ──────────────────────────────────────────────
    let per_call = captured.lock().unwrap().clone();
    assert!(
        per_call.len() >= 2,
        "expected >= 2 stream calls across two turns, got {}",
        per_call.len()
    );

    // First call sees a Write hint on the leading (only) prompt message.
    assert!(
        !per_call[0].is_empty(),
        "first stream call observed zero messages"
    );
    match &per_call[0][0] {
        Some(swink_agent::CacheHint::Write { .. }) => {}
        other => panic!("turn 1 should annotate leading message with Write hint, got {other:?}"),
    }

    // Second call sees a Read hint on the prior leading message — this
    // confirms the prefix survived into turn 2 and was re-annotated.
    assert!(
        !per_call[1].is_empty(),
        "second stream call observed zero messages"
    );
    match &per_call[1][0] {
        Some(swink_agent::CacheHint::Read) => {}
        other => {
            panic!("turn 2 should re-annotate leading message with Read hint, got {other:?}")
        }
    }

    // ── CacheAction event ────────────────────────────────────────────────
    // The regression: before the fix, `prefix_tokens` was always 0. With the
    // fix, turn 1 must report a non-zero prefix.
    let actions = cache_actions(&events);
    assert!(
        !actions.is_empty(),
        "expected at least one CacheAction event (cache eligible)"
    );
    assert!(
        actions[0].1 > 0,
        "turn 1 CacheAction must report prefix_tokens > 0, got {}",
        actions[0].1
    );

    // ── Transformer-visible state ────────────────────────────────────────
    // After the loop ends, the transformer's last-published prefix must be
    // non-zero — proof the turn pipeline propagated the boundary.
    assert!(
        transformer.cached_prefix_len() > 0,
        "sliding-window transformer should have received a non-zero cached prefix, got {}",
        transformer.cached_prefix_len()
    );
}

#[tokio::test]
async fn cache_miss_retry_rewrites_read_hint_to_write() {
    let stream_fn = Arc::new(CaptureHintsStreamFn {
        responses: Mutex::new(vec![
            text_only_events("first"),
            error_events("provider cache miss", None),
            text_only_events("second"),
        ]),
        per_call_hints: Arc::new(Mutex::new(Vec::new())),
    });
    let captured = Arc::clone(&stream_fn.per_call_hints);

    let cache_config = CacheConfig::new(Duration::from_mins(5), 10, 3);
    let mut config = base_config(stream_fn.clone() as Arc<dyn StreamFn>, Some(cache_config));
    config.retry_strategy = Box::new(RetryCacheMissOnce);
    config.message_provider = Some(Arc::new(OneShotFollowUp {
        msg: Mutex::new(Some(user_msg("follow-up question"))),
    }));

    let events = collect_events(agent_loop(
        vec![sized_user_msg("prompt", 50)],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    let per_call = captured.lock().unwrap().clone();
    assert_eq!(
        per_call.len(),
        3,
        "expected turn 1, turn 2 cache-miss attempt, and turn 2 retry"
    );

    match &per_call[0][0] {
        Some(swink_agent::CacheHint::Write { .. }) => {}
        other => panic!("turn 1 should establish the cache with Write, got {other:?}"),
    }
    match &per_call[1][0] {
        Some(swink_agent::CacheHint::Read) => {}
        other => panic!("turn 2 first attempt should read the cache, got {other:?}"),
    }
    match &per_call[2][0] {
        Some(swink_agent::CacheHint::Write { .. }) => {}
        other => panic!("cache-miss retry must refresh the cache with Write, got {other:?}"),
    }

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::AgentEnd { .. })),
        "agent loop should complete after the cache-miss retry"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Regression test: compaction preserves the cached prefix.
// ═══════════════════════════════════════════════════════════════════════════

/// A `ContextTransformer` wrapper that delegates to an inner
/// `SlidingWindowTransformer` and captures the `cached_prefix_len` value
/// **immediately before** each `transform` call. This lets the test observe
/// what the turn pipeline published into the transformer on each turn.
struct ProbingTransformer {
    inner: Arc<SlidingWindowTransformer>,
    observed: Arc<Mutex<Vec<usize>>>,
}

impl swink_agent::ContextTransformer for ProbingTransformer {
    fn transform(
        &self,
        messages: &mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Option<swink_agent::CompactionReport> {
        self.observed
            .lock()
            .unwrap()
            .push(self.inner.cached_prefix_len());
        self.inner.transform(messages, overflow)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        // Forward downcasts to the inner `SlidingWindowTransformer` so the
        // turn pipeline's `publish_cached_prefix` hook still finds a target.
        self.inner.as_ref()
    }
}

#[tokio::test]
async fn compaction_preserves_cached_prefix() {
    let stream_fn = Arc::new(CaptureHintsStreamFn {
        responses: Mutex::new(vec![text_only_events("first"), text_only_events("second")]),
        per_call_hints: Arc::new(Mutex::new(Vec::new())),
    });
    let captured = Arc::clone(&stream_fn.per_call_hints);

    // Low min_tokens so turn 1 records a prefix covering all 3 initial
    // messages.
    let cache_config = CacheConfig::new(Duration::from_mins(5), 10, 5);

    // Normal budget generous enough to avoid compaction on turn 1.
    // Anchor=1 on its own would only preserve the first message; the cache
    // boundary must override that to protect all 3 leading messages.
    let inner = Arc::new(SlidingWindowTransformer::new(10_000, 10_000, 1));
    let observed: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let probing = Arc::new(ProbingTransformer {
        inner: Arc::clone(&inner),
        observed: Arc::clone(&observed),
    });

    let mut config = base_config(stream_fn.clone() as Arc<dyn StreamFn>, Some(cache_config));
    config.transform_context = Some(probing);
    config.message_provider = Some(Arc::new(OneShotFollowUp {
        msg: Mutex::new(Some(user_msg("follow-up"))),
    }));

    let initial_messages = vec![
        sized_user_msg("ANCHOR_A", 20),
        sized_user_msg("ANCHOR_B", 20),
        sized_user_msg("ANCHOR_C", 20),
    ];

    let _events = collect_events(agent_loop(
        initial_messages,
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    // Turn 1: transformer saw prefix=0 (nothing cached yet).
    // Turn 2: transformer saw the turn-1 prefix (>= 3 leading messages).
    let observed = observed.lock().unwrap().clone();
    assert!(
        observed.len() >= 2,
        "transformer should have been invoked on both turns, got {}",
        observed.len()
    );
    assert_eq!(observed[0], 0, "turn 1 should start with prefix 0");
    assert!(
        observed[1] >= 3,
        "turn 2 must see cached prefix >= 3 (protecting all 3 anchor messages), got {}",
        observed[1]
    );

    // The stream saw all 3 anchor messages plus the assistant reply and the
    // follow-up on turn 2 — compaction did not drop any leading message.
    let per_call = captured.lock().unwrap().clone();
    assert!(per_call.len() >= 2, "expected 2 stream calls");
    assert!(
        per_call[1].len() >= 3,
        "turn 2 must still include >= 3 leading messages after compaction, got {}",
        per_call[1].len()
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Pre-fix regression smoke: the raw `CacheState` must record a non-zero prefix.
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cache_state_records_non_zero_prefix_after_write_turn() {
    let stream_fn = Arc::new(CaptureHintsStreamFn {
        responses: Mutex::new(vec![text_only_events("ok")]),
        per_call_hints: Arc::new(Mutex::new(Vec::new())),
    });

    let cache_config = CacheConfig::new(Duration::from_mins(5), 10, 3);
    let mut config = base_config(stream_fn as Arc<dyn StreamFn>, Some(cache_config));
    config.transform_context = Some(Arc::new(SlidingWindowTransformer::new(10_000, 5_000, 1)));

    let events = collect_events(agent_loop(
        vec![sized_user_msg("prompt", 30)],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    // CacheAction must fire on the Write turn with non-zero prefix tokens.
    let actions = cache_actions(&events);
    assert_eq!(
        actions.len(),
        1,
        "exactly one CacheAction expected on a single-turn run, got {}",
        actions.len()
    );
    assert!(
        matches!(actions[0].0, swink_agent::CacheHint::Write { .. }),
        "first turn hint must be Write, got {:?}",
        actions[0].0
    );
    assert!(
        actions[0].1 > 0,
        "prefix_tokens must be non-zero on the Write turn (was 0 before the fix)"
    );

    // The terminal stop reason must still be a normal End — caching should
    // not change overall control flow.
    let saw_end = events.iter().any(|e| {
        matches!(
            e,
            AgentEvent::TurnEnd {
                assistant_message,
                ..
            } if matches!(
                assistant_message.stop_reason,
                StopReason::Stop | StopReason::ToolUse
            )
        )
    });
    assert!(saw_end, "expected a TurnEnd event with normal stop reason");
}
