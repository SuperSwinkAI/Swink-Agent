#![cfg(feature = "testkit")]
//! Integration tests for manual context compaction (`Agent::compact_context`).
//!
//! Covers the acceptance criteria of issue #1102: on-demand pruning between
//! turns, under-budget no-op, no-transformer no-op, event observation via
//! subscribers and event forwarders, and the running-loop guard.

mod common;

use std::pin::Pin;
use std::sync::{Arc, Mutex};

use common::{
    EventCollector, default_convert, default_exhausted_fallback, default_model, text_only_events,
    user_msg,
};
use futures::Stream;
use tokio_util::sync::CancellationToken;

use swink_agent::testing::next_response;
use swink_agent::{
    Agent, AgentContext, AgentError, AgentEvent, AgentMessage, AgentOptions, AssistantMessageEvent,
    AsyncContextTransformer, AsyncTransformFuture, CompactionReport, ContentBlock, LlmMessage,
    ModelSpec, SlidingWindowTransformer, StreamFn, StreamOptions, UserMessage,
};

// ─── Helpers ─────────────────────────────────────────────────────────────

/// A `StreamFn` that captures the full LLM message list on each call.
struct MessageCapturingStreamFn {
    responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
    captured: Arc<Mutex<Vec<Vec<LlmMessage>>>>,
}

impl MessageCapturingStreamFn {
    fn new(
        responses: Vec<Vec<AssistantMessageEvent>>,
    ) -> (Arc<Self>, Arc<Mutex<Vec<Vec<LlmMessage>>>>) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let this = Arc::new(Self {
            responses: Mutex::new(responses),
            captured: Arc::clone(&captured),
        });
        (this, captured)
    }
}

impl StreamFn for MessageCapturingStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let llm_msgs: Vec<LlmMessage> = context
            .messages
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(llm) => Some(llm.clone()),
                AgentMessage::Custom(_) => None,
            })
            .collect();
        self.captured.lock().unwrap().push(llm_msgs);
        let events = next_response(&self.responses, default_exhausted_fallback());
        Box::pin(futures::stream::iter(events))
    }
}

/// Create a large user message (~`token_count` estimated tokens; chars / 4).
fn large_user_msg(label: &str, token_count: usize) -> AgentMessage {
    let padding = "x".repeat(token_count * 4);
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: format!("{label}:{padding}"),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

/// Seed a history of `n` large messages (~50 estimated tokens each).
fn seed_large_history(agent: &mut Agent, n: usize) {
    let msgs: Vec<AgentMessage> = (0..n)
        .map(|i| large_user_msg(&format!("m{i}"), 50))
        .collect();
    agent.set_messages(msgs);
}

fn first_text(msg: &LlmMessage) -> Option<&str> {
    let content = match msg {
        LlmMessage::User(u) => &u.content,
        _ => return None,
    };
    content.iter().find_map(|b| match b {
        ContentBlock::Text { text } => Some(text.as_str()),
        _ => None,
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// AC 1: long history + transformer — prunes, returns report, next request
// contains the pruned message set
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn manual_compaction_prunes_and_next_request_reflects_it() {
    let (stream_fn, captured) = MessageCapturingStreamFn::new(vec![text_only_events("reply")]);

    let mut agent = Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            stream_fn as Arc<dyn StreamFn>,
            default_convert,
        )
        // normal_budget=10_000 so the loop itself would never compact;
        // overflow_budget=150 so manual (overflow=true) compaction prunes hard.
        .with_transform_context(SlidingWindowTransformer::new(10_000, 150, 1)),
    );

    // 10 messages * ~50 tokens = ~500 tokens, well over the 150 overflow budget.
    seed_large_history(&mut agent, 10);
    let before = agent.state().messages.len();

    let report = agent
        .compact_context()
        .await
        .expect("agent is idle")
        .expect("history over budget must compact");

    let after = agent.state().messages.len();
    assert!(after < before, "history must shrink ({before} -> {after})");
    assert_eq!(report.dropped_count, before - after);
    assert!(report.overflow, "manual compaction runs with overflow=true");
    assert!(report.tokens_after < report.tokens_before);

    // The next request must contain exactly the pruned message set.
    agent
        .prompt_async(vec![user_msg("follow-up")])
        .await
        .unwrap();
    let captured = captured.lock().unwrap();
    let sent = &captured[0];
    assert_eq!(
        sent.len(),
        after + 1,
        "next request = pruned history + new user message"
    );
    // Anchor (m0) survives, the new prompt is last, and dropped middle
    // messages (e.g. m1) are absent.
    assert!(first_text(&sent[0]).is_some_and(|t| t.starts_with("m0:")));
    assert!(first_text(sent.last().unwrap()).is_some_and(|t| t == "follow-up"));
    assert!(
        !sent
            .iter()
            .any(|m| first_text(m).is_some_and(|t| t.starts_with("m1:"))),
        "dropped messages must not reach the provider"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// AC 2: short history under budget — transformer declines — Ok(None)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn manual_compaction_under_budget_is_noop() {
    let (stream_fn, _captured) = MessageCapturingStreamFn::new(vec![]);
    let collector = EventCollector::new();

    let mut agent = Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            stream_fn as Arc<dyn StreamFn>,
            default_convert,
        )
        .with_transform_context(SlidingWindowTransformer::new(10_000, 5_000, 1)),
    );
    agent.subscribe(collector.subscriber());

    // 3 messages * ~50 tokens — far under the 5_000 overflow budget.
    seed_large_history(&mut agent, 3);
    let before = agent.state().messages.len();

    let report = agent.compact_context().await.expect("agent is idle");

    // The sliding window returns None when it declines (same as in the loop),
    // so manual compaction reports Ok(None) — not Some with dropped_count == 0.
    assert!(report.is_none(), "under-budget compaction is a no-op");
    assert_eq!(agent.state().messages.len(), before);
    assert_eq!(collector.count(), 0, "no event for a declined compaction");
}

// ═══════════════════════════════════════════════════════════════════════════
// AC 3: no transformer configured — Ok(None), no event emitted
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn manual_compaction_without_transformer_returns_none_and_no_event() {
    let (stream_fn, _captured) = MessageCapturingStreamFn::new(vec![]);
    let collector = EventCollector::new();

    let mut agent = Agent::new(AgentOptions::new(
        "test system prompt",
        default_model(),
        stream_fn as Arc<dyn StreamFn>,
        default_convert,
    ));
    agent.subscribe(collector.subscriber());

    seed_large_history(&mut agent, 10);
    let before = agent.state().messages.len();

    let report = agent.compact_context().await.expect("agent is idle");

    assert!(report.is_none(), "no transformer => None");
    assert_eq!(agent.state().messages.len(), before, "history untouched");
    assert_eq!(collector.count(), 0, "no event emitted");
}

// ═══════════════════════════════════════════════════════════════════════════
// AC 4: ContextCompacted observed by a subscribed host — listener AND
// event forwarder (the TUI consumes forwarders)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn manual_compaction_event_reaches_listener_and_forwarder() {
    let (stream_fn, _captured) = MessageCapturingStreamFn::new(vec![]);
    let collector = EventCollector::new();
    let forwarded: Arc<Mutex<Vec<CompactionReport>>> = Arc::new(Mutex::new(Vec::new()));
    let forwarded_clone = Arc::clone(&forwarded);

    let mut agent = Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            stream_fn as Arc<dyn StreamFn>,
            default_convert,
        )
        .with_transform_context(SlidingWindowTransformer::new(10_000, 150, 1))
        .with_event_forwarder(move |event| {
            if let AgentEvent::ContextCompacted { report } = event {
                forwarded_clone.lock().unwrap().push(report);
            }
        }),
    );
    agent.subscribe(collector.subscriber());

    seed_large_history(&mut agent, 10);

    let report = agent
        .compact_context()
        .await
        .expect("agent is idle")
        .expect("must compact");

    // Listener saw it.
    assert_eq!(
        collector.events(),
        vec!["ContextCompacted".to_string()],
        "listener must observe exactly one ContextCompacted"
    );
    // Forwarder saw the identical report.
    let forwarded = forwarded.lock().unwrap();
    assert_eq!(forwarded.len(), 1, "forwarder must observe the event");
    assert_eq!(forwarded[0].dropped_count, report.dropped_count);
    assert!(forwarded[0].overflow);
}

// ═══════════════════════════════════════════════════════════════════════════
// Both transformers: async first, sync second, one event each, last report
// returned
// ═══════════════════════════════════════════════════════════════════════════

/// Async transformer that drops the first message and tags the report with
/// a sentinel token count so tests can tell the two reports apart.
struct DropFirstAsyncTransformer;

impl AsyncContextTransformer for DropFirstAsyncTransformer {
    fn transform<'a>(
        &'a self,
        messages: &'a mut Vec<AgentMessage>,
        overflow: bool,
    ) -> AsyncTransformFuture<'a> {
        Box::pin(async move {
            assert!(overflow, "manual compaction must pass overflow=true");
            if messages.len() <= 1 {
                return None;
            }
            messages.remove(0);
            Some(CompactionReport {
                dropped_count: 1,
                tokens_before: 777, // sentinel: identifies the async report
                tokens_after: 0,
                overflow,
                dropped_messages: Vec::new(),
            })
        })
    }
}

#[tokio::test]
async fn manual_compaction_runs_async_then_sync_and_returns_last_report() {
    let (stream_fn, _captured) = MessageCapturingStreamFn::new(vec![]);
    let collector = EventCollector::new();

    let mut agent = Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            stream_fn as Arc<dyn StreamFn>,
            default_convert,
        )
        .with_async_transform_context(DropFirstAsyncTransformer)
        .with_transform_context(SlidingWindowTransformer::new(10_000, 150, 1)),
    );
    agent.subscribe(collector.subscriber());

    seed_large_history(&mut agent, 10);
    let before = agent.state().messages.len();

    let report = agent
        .compact_context()
        .await
        .expect("agent is idle")
        .expect("both transformers compact");

    // One event per compacting transformer, in loop order.
    assert_eq!(
        collector.events(),
        vec![
            "ContextCompacted".to_string(),
            "ContextCompacted".to_string()
        ],
        "async and sync transformer must each emit one event"
    );
    // The returned report is the sync (last) transformer's, not the async
    // sentinel.
    assert_ne!(
        report.tokens_before, 777,
        "returned report must come from the last (sync) transformer"
    );
    assert!(agent.state().messages.len() < before - 1, "both pruned");
}

// ═══════════════════════════════════════════════════════════════════════════
// Guard: compaction while a loop is active returns AlreadyRunning
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn manual_compaction_while_running_returns_already_running() {
    let (stream_fn, _captured) = MessageCapturingStreamFn::new(vec![text_only_events("hello")]);

    let mut agent = Agent::new(
        AgentOptions::new(
            "test system prompt",
            default_model(),
            stream_fn as Arc<dyn StreamFn>,
            default_convert,
        )
        .with_transform_context(SlidingWindowTransformer::new(10_000, 150, 1)),
    );

    seed_large_history(&mut agent, 10);

    // Start a run and keep the stream alive — the loop is active.
    let stream = agent.prompt_stream(vec![user_msg("hi")]).unwrap();
    assert!(agent.is_running());

    let err = agent.compact_context().await;
    assert!(
        matches!(err, Err(AgentError::AlreadyRunning)),
        "expected AlreadyRunning while the loop is active, got {err:?}"
    );

    // Dropping the stream makes the agent idle; the guard clears. Dropping
    // an un-drained `prompt_stream` no longer empties the history:
    // `start_loop` keeps a snapshot in `state.messages`, so the seeded
    // history (plus the "hi" prompt) survives the drop and compaction goes
    // through without re-seeding.
    drop(stream);
    assert!(!agent.is_running());
    assert_eq!(
        agent.state().messages.len(),
        11,
        "history (10 seeded + 1 prompt) must survive dropping an un-drained stream"
    );
    let report = agent.compact_context().await.expect("agent is idle now");
    assert!(report.is_some(), "compaction succeeds once idle");
}
