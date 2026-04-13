#![cfg(feature = "testkit")]
//! Regression test for #431: aborted provider stops must emit
//! `TurnEndReason::Aborted`, not `TurnEndReason::Error`.

mod common;

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use common::{MockStreamFn, default_model, text_only_events, user_msg};
use futures::Stream;
use futures::stream::StreamExt;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentEvent, AgentLoopConfig, AgentMessage, AssistantMessageEvent, Cost, DefaultRetryStrategy,
    LlmMessage, StopReason, StreamFn, StreamOptions, TurnEndReason, Usage, agent_loop,
};

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

async fn collect_events(stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>) -> Vec<AgentEvent> {
    stream.collect().await
}

/// Build a stream event sequence that terminates with `StopReason::Aborted`.
fn aborted_events() -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "partial".to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Aborted,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ]
}

/// Build a stream event sequence that terminates with `StopReason::Error`.
fn error_stop_events() -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "oops".to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Error,
            usage: Usage::default(),
            cost: Cost::default(),
        },
    ]
}

// ═══════════════════════════════════════════════════════════════════════════════
// Regression: aborted stop emits TurnEndReason::Aborted (#431)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn aborted_stop_emits_turn_end_reason_aborted() {
    let stream_fn: Arc<dyn swink_agent::StreamFn> =
        Arc::new(MockStreamFn::new(vec![aborted_events()]));
    let config = default_config(stream_fn);

    let events = collect_events(agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    let turn_end = events
        .iter()
        .find(|e| matches!(e, AgentEvent::TurnEnd { .. }));
    assert!(turn_end.is_some(), "expected a TurnEnd event");

    match turn_end.unwrap() {
        AgentEvent::TurnEnd { reason, .. } => {
            assert_eq!(
                *reason,
                TurnEndReason::Aborted,
                "aborted stop should produce TurnEndReason::Aborted, not {:?}",
                reason
            );
        }
        _ => unreachable!(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Sanity: genuine error stop still emits TurnEndReason::Error
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn error_stop_still_emits_turn_end_reason_error() {
    let stream_fn: Arc<dyn swink_agent::StreamFn> =
        Arc::new(MockStreamFn::new(vec![error_stop_events()]));
    let config = default_config(stream_fn);

    let events = collect_events(agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    let turn_end = events
        .iter()
        .find(|e| matches!(e, AgentEvent::TurnEnd { .. }));
    assert!(turn_end.is_some(), "expected a TurnEnd event");

    match turn_end.unwrap() {
        AgentEvent::TurnEnd { reason, .. } => {
            assert_eq!(
                *reason,
                TurnEndReason::Error,
                "error stop should produce TurnEndReason::Error, not {:?}",
                reason
            );
        }
        _ => unreachable!(),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Sanity: normal stop still emits TurnEndReason::Complete
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn normal_stop_emits_turn_end_reason_complete() {
    let stream_fn: Arc<dyn swink_agent::StreamFn> =
        Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let config = default_config(stream_fn);

    let events = collect_events(agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    ))
    .await;

    let turn_end = events
        .iter()
        .find(|e| matches!(e, AgentEvent::TurnEnd { .. }));
    assert!(turn_end.is_some(), "expected a TurnEnd event");

    match turn_end.unwrap() {
        AgentEvent::TurnEnd { reason, .. } => {
            assert_eq!(
                *reason,
                TurnEndReason::Complete,
                "normal stop should produce TurnEndReason::Complete, not {:?}",
                reason
            );
        }
        _ => unreachable!(),
    }
}
