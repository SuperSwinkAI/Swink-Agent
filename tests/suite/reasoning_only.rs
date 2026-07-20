//! Tests for #1195: guard for reasoning-only final turns.
//!
//! A model can end its turn with only hidden-channel reasoning — no visible
//! text, no tool call. The loop must (a) always signal this structurally via
//! `TurnEndReason::ReasoningOnly`, and (b) when the opt-in nudge is enabled,
//! inject one corrective reminder and retry exactly once before accepting.

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use crate::common::{
    MockStreamFn, default_model, text_only_events, thinking_only_events, user_msg,
};
use futures::Stream;
use futures::stream::StreamExt;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentEvent, AgentLoopConfig, AgentMessage, AssistantMessage, AssistantMessageEvent,
    ContentBlock, Cost, DefaultRetryStrategy, LlmMessage, StopReason, StreamFn, TurnEndReason,
    Usage, agent_loop,
};

type ConvertToLlmBoxed = Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync>;

fn default_convert_to_llm() -> ConvertToLlmBoxed {
    Box::new(|msg: &AgentMessage| match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        // Covers AgentMessage::Custom and, since AgentMessage is
        // #[non_exhaustive], any future variant.
        _ => None,
    })
}

fn default_config(stream_fn: Arc<dyn StreamFn>) -> AgentLoopConfig {
    let mut config = AgentLoopConfig::new(default_model(), stream_fn, default_convert_to_llm());
    config.retry_strategy = Box::new(
        DefaultRetryStrategy::default()
            .with_jitter(false)
            .with_base_delay(Duration::from_millis(1)),
    );
    config
}

async fn collect_events(stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>) -> Vec<AgentEvent> {
    stream.collect().await
}

fn turn_end_reasons(events: &[AgentEvent]) -> Vec<TurnEndReason> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnEnd { reason, .. } => Some(*reason),
            _ => None,
        })
        .collect()
}

fn count_turn_starts(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart))
        .count()
}

/// Events for a response mixing a thinking block with visible text.
fn thinking_then_text_events() -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ThinkingStart { content_index: 0 },
        AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "pondering".to_string(),
        },
        AssistantMessageEvent::ThinkingEnd {
            content_index: 0,
            signature: None,
        },
        AssistantMessageEvent::TextStart { content_index: 1 },
        AssistantMessageEvent::TextDelta {
            content_index: 1,
            delta: "visible answer".to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 1 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ]
}

// ═══════════════════════════════════════════════════════════════════════════════
// Structural signal (always on)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn reasoning_only_turn_emits_reasoning_only_reason() {
    let stream_fn: Arc<dyn StreamFn> =
        Arc::new(MockStreamFn::new(vec![thinking_only_events("hmm...")]));
    let config = default_config(stream_fn);

    let events = collect_events(agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert_eq!(
        turn_end_reasons(&events),
        vec![TurnEndReason::ReasoningOnly]
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. })),
        "loop must still terminate normally"
    );
}

#[tokio::test]
async fn thinking_plus_visible_text_completes_normally() {
    let stream_fn: Arc<dyn StreamFn> =
        Arc::new(MockStreamFn::new(vec![thinking_then_text_events()]));
    let config = default_config(stream_fn);

    let events = collect_events(agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert_eq!(turn_end_reasons(&events), vec![TurnEndReason::Complete]);
}

#[tokio::test]
async fn nudge_disabled_by_default_accepts_without_retry() {
    let stream_fn: Arc<dyn StreamFn> =
        Arc::new(MockStreamFn::new(vec![thinking_only_events("hmm...")]));
    let config = default_config(stream_fn);
    assert!(!config.reasoning_only_nudge, "nudge must default to off");

    let events = collect_events(agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    // Exactly one LLM call: no hidden retry when the nudge is off.
    assert_eq!(count_turn_starts(&events), 1);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Opt-in nudge (one retry per occurrence)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn nudge_retries_once_then_accepts_visible_reply() {
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![
        thinking_only_events("hmm..."),
        text_only_events("Here is the visible answer."),
    ]));
    let mut config = default_config(stream_fn);
    config.reasoning_only_nudge = true;

    let events = collect_events(agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    assert_eq!(count_turn_starts(&events), 2, "exactly one retry");
    assert_eq!(
        turn_end_reasons(&events),
        vec![TurnEndReason::ReasoningOnly, TurnEndReason::Complete]
    );

    // The corrective reminder must be part of the final context.
    let messages = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::AgentEnd { messages } => Some(Arc::clone(messages)),
            _ => None,
        })
        .expect("AgentEnd event");
    let nudge_present = messages.iter().any(|m| match m {
        AgentMessage::Llm(LlmMessage::User(user)) => user.content.iter().any(
            |b| matches!(b, ContentBlock::Text { text } if text.contains("only hidden reasoning")),
        ),
        _ => false,
    });
    assert!(nudge_present, "nudge reminder should be in the context");
}

#[tokio::test]
async fn nudge_gives_up_after_single_retry() {
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![
        thinking_only_events("hmm..."),
        thinking_only_events("still hmm..."),
    ]));
    let mut config = default_config(stream_fn);
    config.reasoning_only_nudge = true;

    let events = collect_events(agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    // Two LLM calls, then acceptance — never a third.
    assert_eq!(count_turn_starts(&events), 2);
    assert_eq!(
        turn_end_reasons(&events),
        vec![TurnEndReason::ReasoningOnly, TurnEndReason::ReasoningOnly]
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. })),
        "loop must terminate after accepting the retried turn"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// AssistantMessage helpers
// ═══════════════════════════════════════════════════════════════════════════════

fn assistant(content: Vec<ContentBlock>) -> AssistantMessage {
    AssistantMessage::new(content, "test", "test-model")
}

#[test]
fn helper_classifies_reasoning_only() {
    let msg = assistant(vec![ContentBlock::Thinking {
        thinking: "pondering".to_string(),
        signature: None,
    }]);
    assert!(!msg.has_visible_content());
    assert!(msg.is_reasoning_only());
}

#[test]
fn helper_treats_whitespace_text_as_invisible() {
    let msg = assistant(vec![
        ContentBlock::Thinking {
            thinking: "pondering".to_string(),
            signature: None,
        },
        ContentBlock::Text {
            text: "  \n\t ".to_string(),
        },
    ]);
    assert!(msg.is_reasoning_only());
}

#[test]
fn helper_counts_text_and_tool_calls_as_visible() {
    let with_text = assistant(vec![ContentBlock::Text {
        text: "hi".to_string(),
    }]);
    assert!(with_text.has_visible_content());
    assert!(!with_text.is_reasoning_only());

    let with_tool = assistant(vec![
        ContentBlock::Thinking {
            thinking: "pondering".to_string(),
            signature: None,
        },
        ContentBlock::ToolCall {
            id: "tc_1".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({}),
            partial_json: None,
        },
    ]);
    assert!(with_tool.has_visible_content());
    assert!(!with_tool.is_reasoning_only());
}

#[test]
fn helper_empty_message_is_not_reasoning_only() {
    let empty = assistant(vec![]);
    assert!(!empty.has_visible_content());
    assert!(
        !empty.is_reasoning_only(),
        "no reasoning present — plain empty is a different failure"
    );
}
