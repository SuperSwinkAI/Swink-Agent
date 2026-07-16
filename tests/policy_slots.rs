#![cfg(feature = "testkit")]
//! Integration tests for the `PostTurn` and `PostLoop` policy slots (spec
//! 031-policy-slots), exercised exclusively through the public
//! `AgentOptions::with_post_turn_policy` / `with_post_loop_policy` builder
//! methods and `Agent::prompt_async`.
//!
//! Prior to this file, `PostTurnPolicy` and `PostLoopPolicy` were only ever
//! constructed by directly assigning `AgentLoopConfig` fields (see
//! `tests/agent_loop.rs`, `tests/loop_overflow.rs`) or via `Plugin`
//! contributions (`tests/plugin_integration.rs`) — the public builder API
//! itself had zero coverage.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{MockStreamFn, default_convert, default_model, text_only_events, user_msg};

use swink_agent::{
    Agent, AgentMessage, AgentOptions, AssistantMessage, AssistantMessageEvent, Cost, LlmMessage,
    PolicyContext, PolicyVerdict, PostLoopPolicy, PostTurnPolicy, StopReason, TurnPolicyContext,
    Usage,
};

// ─── Helpers ─────────────────────────────────────────────────────────────

fn done_with_usage(usage: Usage, cost: Cost) -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "scripted reply".to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage,
            cost,
        },
    ]
}

fn assistant_text(message: &AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|b| match b {
            swink_agent::ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

// ═════════════════════════════════════════════════════════════════════════
// PostTurnPolicy: fires through the public builder and observes turn context
// ═════════════════════════════════════════════════════════════════════════

/// Snapshot of everything a `PostTurnPolicy` observed on its most recent
/// evaluation. The policy holds a shared handle to this record; the test
/// keeps a clone of the same `Arc` to inspect after `prompt_async` returns
/// (the policy struct itself is moved by value into the builder, matching
/// the shared-handle convention used by `src/testing.rs`'s
/// `RecordingPostTurnPolicy`).
#[derive(Default)]
struct PostTurnRecord {
    calls: usize,
    last_turn_index: Option<usize>,
    last_assistant_text: Option<String>,
    last_stop_reason: Option<StopReason>,
    last_system_prompt: Option<String>,
}

struct RecordingPostTurn {
    record: Arc<Mutex<PostTurnRecord>>,
}

impl PostTurnPolicy for RecordingPostTurn {
    fn name(&self) -> &str {
        "recording-post-turn"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let mut record = self.record.lock().unwrap();
        record.calls += 1;
        record.last_turn_index = Some(ctx.turn_index);
        record.last_assistant_text = Some(assistant_text(turn.assistant_message));
        record.last_stop_reason = Some(turn.stop_reason);
        record.last_system_prompt = Some(turn.system_prompt.to_string());
        PolicyVerdict::Continue
    }
}

#[tokio::test]
async fn post_turn_policy_fires_via_public_builder_and_observes_turn_context() {
    let record = Arc::new(Mutex::new(PostTurnRecord::default()));
    let policy = RecordingPostTurn {
        record: Arc::clone(&record),
    };
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello there")]));

    let options = AgentOptions::new("be terse", default_model(), stream_fn, default_convert)
        .with_post_turn_policy(policy);
    let mut agent = Agent::new(options);

    let _ = agent
        .prompt_async(vec![user_msg("hi")])
        .await
        .expect("prompt_async should succeed");

    let record = record.lock().unwrap();
    assert_eq!(
        record.calls, 1,
        "post-turn policy registered via with_post_turn_policy should fire exactly once for a single-turn response"
    );
    assert_eq!(
        record.last_turn_index,
        Some(0),
        "post-turn policy should observe the turn index of the completed turn"
    );
    assert_eq!(
        record.last_assistant_text.as_deref(),
        Some("hello there"),
        "post-turn policy should observe the actual assistant text produced by the turn"
    );
    assert_eq!(
        record.last_stop_reason,
        Some(StopReason::Stop),
        "post-turn policy should observe the real stop reason"
    );
    assert_eq!(
        record.last_system_prompt.as_deref(),
        Some("be terse"),
        "post-turn policy should observe the system prompt configured on AgentOptions"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// PostTurnPolicy: Inject(AssistantMessage) replaces the committed message
// ═════════════════════════════════════════════════════════════════════════

struct ReplacingPostTurn {
    replacement_text: String,
}

impl PostTurnPolicy for ReplacingPostTurn {
    fn name(&self) -> &str {
        "replacing-post-turn"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        let mut replacement = turn.assistant_message.clone();
        replacement.content = vec![swink_agent::ContentBlock::Text {
            text: self.replacement_text.clone(),
        }];
        PolicyVerdict::Inject(vec![AgentMessage::Llm(LlmMessage::Assistant(replacement))])
    }
}

#[tokio::test]
async fn post_turn_policy_inject_replaces_committed_assistant_message() {
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("original reply")]));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_post_turn_policy(ReplacingPostTurn {
            replacement_text: "redacted by policy".to_string(),
        });
    let mut agent = Agent::new(options);

    let result = agent
        .prompt_async(vec![user_msg("hi")])
        .await
        .expect("prompt_async should succeed");

    let committed_text: String = result
        .messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(LlmMessage::Assistant(a)) => Some(assistant_text(a)),
            _ => None,
        })
        .collect();

    assert_eq!(
        committed_text, "redacted by policy",
        "the assistant message committed to history should be the policy's replacement, not the original mock output"
    );
    assert!(
        !committed_text.contains("original reply"),
        "the original scripted text must not survive an Inject(AssistantMessage) replacement"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// PostLoopPolicy: fires through the public builder, exactly once per call
// ═════════════════════════════════════════════════════════════════════════

struct CountingPostLoop {
    calls: Arc<AtomicUsize>,
}

impl PostLoopPolicy for CountingPostLoop {
    fn name(&self) -> &str {
        "counting-post-loop"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>) -> PolicyVerdict {
        self.calls.fetch_add(1, Ordering::SeqCst);
        PolicyVerdict::Continue
    }
}

#[tokio::test]
async fn post_loop_policy_fires_exactly_once_via_public_builder() {
    let calls = Arc::new(AtomicUsize::new(0));
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));

    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_post_loop_policy(CountingPostLoop {
            calls: Arc::clone(&calls),
        });
    let mut agent = Agent::new(options);

    let _ = agent
        .prompt_async(vec![user_msg("hi")])
        .await
        .expect("prompt_async should succeed");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "post-loop policy registered via with_post_loop_policy should fire exactly once after the (single-iteration) outer loop exits"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// PostLoopPolicy: observes finalized accumulated state after the loop ends
// ═════════════════════════════════════════════════════════════════════════

#[derive(Default)]
struct PostLoopRecord {
    turn_index: Option<usize>,
    usage: Option<Usage>,
    cost_total: Option<f64>,
}

struct RecordingPostLoop {
    record: Arc<Mutex<PostLoopRecord>>,
}

impl PostLoopPolicy for RecordingPostLoop {
    fn name(&self) -> &str {
        "recording-post-loop"
    }

    fn evaluate(&self, ctx: &PolicyContext<'_>) -> PolicyVerdict {
        let mut record = self.record.lock().unwrap();
        record.turn_index = Some(ctx.turn_index);
        record.usage = Some(ctx.accumulated_usage.clone());
        record.cost_total = Some(ctx.accumulated_cost.total);
        PolicyVerdict::Continue
    }
}

#[tokio::test]
async fn post_loop_policy_observes_final_accumulated_state() {
    let scripted_usage = Usage::default()
        .with_input(100)
        .with_output(42)
        .with_total(142);
    let scripted_cost = Cost::default().with_total(3.5);
    let stream_fn = Arc::new(MockStreamFn::new(vec![done_with_usage(
        scripted_usage.clone(),
        scripted_cost.clone(),
    )]));

    let record = Arc::new(Mutex::new(PostLoopRecord::default()));
    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_post_loop_policy(RecordingPostLoop {
            record: Arc::clone(&record),
        });
    let mut agent = Agent::new(options);

    let _ = agent
        .prompt_async(vec![user_msg("hi")])
        .await
        .expect("prompt_async should succeed");

    let record = record.lock().unwrap();
    assert_eq!(
        record.turn_index,
        Some(1),
        "after a single completed turn, turn_index should have been incremented to 1 by the time PostLoop runs"
    );
    assert_eq!(
        record.usage.as_ref(),
        Some(&scripted_usage),
        "PostLoop should observe the real accumulated usage from the completed turn, not a placeholder"
    );
    assert_eq!(
        record.cost_total,
        Some(3.5),
        "PostLoop should observe the real accumulated cost from the completed turn"
    );
}

// ═════════════════════════════════════════════════════════════════════════
// Cross-slot interaction: a PostTurn Stop prevents PostLoop from ever running
// ═════════════════════════════════════════════════════════════════════════

struct StoppingPostTurn;

impl PostTurnPolicy for StoppingPostTurn {
    fn name(&self) -> &str {
        "stopping-post-turn"
    }

    fn evaluate(&self, _ctx: &PolicyContext<'_>, _turn: &TurnPolicyContext<'_>) -> PolicyVerdict {
        PolicyVerdict::Stop("halted by post-turn policy".to_string())
    }
}

#[tokio::test]
async fn post_turn_stop_short_circuits_before_post_loop_runs() {
    let post_loop_calls = Arc::new(AtomicUsize::new(0));
    let stream_fn = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));

    let options = AgentOptions::new("test", default_model(), stream_fn, default_convert)
        .with_post_turn_policy(StoppingPostTurn)
        .with_post_loop_policy(CountingPostLoop {
            calls: Arc::clone(&post_loop_calls),
        });
    let mut agent = Agent::new(options);

    let _ = agent.prompt_async(vec![user_msg("hi")]).await;

    assert_eq!(
        post_loop_calls.load(Ordering::SeqCst),
        0,
        "PostTurn::Stop returns from the agent loop before the inner loop breaks, so PostLoop policies must never run"
    );
}
