#![cfg(feature = "testkit")]
mod common;

use std::sync::Arc;
use std::time::Duration;

use futures::{Stream, StreamExt};
use std::pin::Pin;
use swink_agent::{
    AgentEvent, AgentLoopConfig, AgentMessage, AssistantMessageEvent, DefaultRetryStrategy,
    LlmMessage, ModelFallback, ModelSpec, StopReason, StreamFn, StreamOptions, agent_loop,
};
use tokio_util::sync::CancellationToken;

use common::{MockStreamFn, text_only_events, user_msg};

type ConvertToLlmBoxed = Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync>;

fn primary_model() -> ModelSpec {
    ModelSpec::new("test", "primary-model")
}

fn fallback_model() -> ModelSpec {
    ModelSpec::new("test", "fallback-model")
}

fn default_convert_to_llm() -> ConvertToLlmBoxed {
    Box::new(|msg| match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    })
}

fn error_events(error_message: &str) -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: error_message.to_string(),
            error_kind: None,
            usage: None,
        },
    ]
}

fn default_config(
    stream_fn: Arc<dyn StreamFn>,
    fallback: Option<ModelFallback>,
) -> AgentLoopConfig {
    AgentLoopConfig {
        agent_name: None,
        transfer_chain: None,
        model: primary_model(),
        stream_options: StreamOptions::default(),
        retry_strategy: Box::new(
            DefaultRetryStrategy::default()
                .with_max_attempts(1) // exhaust immediately
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
        fallback,
        tool_execution_policy: swink_agent::ToolExecutionPolicy::default(),
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

async fn collect_events(stream: Pin<Box<dyn Stream<Item = AgentEvent> + Send>>) -> Vec<AgentEvent> {
    stream.collect().await
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn fallback_triggers_on_retryable_error() {
    // Primary model returns rate limit error (retryable).
    // With max_attempts=1, retries exhaust immediately.
    // Fallback model succeeds.
    let primary_stream = Arc::new(MockStreamFn::new(vec![error_events(
        "rate limit exceeded 429",
    )]));
    let fallback_stream = Arc::new(MockStreamFn::new(vec![text_only_events(
        "fallback response",
    )]));

    let fallback = ModelFallback::new(vec![(
        fallback_model(),
        fallback_stream as Arc<dyn StreamFn>,
    )]);

    let config = default_config(primary_stream, Some(fallback));
    let stream = agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );

    let events = collect_events(stream).await;

    // Should have a ModelFallback event
    let has_fallback = events
        .iter()
        .any(|e| matches!(e, AgentEvent::ModelFallback { .. }));
    assert!(has_fallback, "expected ModelFallback event");

    // Should have a successful message from fallback
    let has_message_end = events.iter().any(|e| {
        if let AgentEvent::MessageEnd { message } = e {
            message.stop_reason == StopReason::Stop
        } else {
            false
        }
    });
    assert!(
        has_message_end,
        "expected successful MessageEnd from fallback model"
    );
}

#[tokio::test]
async fn no_fallback_on_non_retryable_error() {
    // Primary model returns a non-retryable error (stream error, not throttled).
    // Even with fallback configured, non-retryable errors should NOT trigger fallback.
    let primary_stream = Arc::new(MockStreamFn::new(vec![error_events(
        "some internal server error",
    )]));
    let fallback_stream = Arc::new(MockStreamFn::new(vec![text_only_events(
        "fallback response",
    )]));

    let fallback = ModelFallback::new(vec![(
        fallback_model(),
        fallback_stream as Arc<dyn StreamFn>,
    )]);

    let config = default_config(primary_stream, Some(fallback));
    let stream = agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );

    let events = collect_events(stream).await;

    // Should NOT have a ModelFallback event
    let has_fallback = events
        .iter()
        .any(|e| matches!(e, AgentEvent::ModelFallback { .. }));
    assert!(!has_fallback, "should not fallback on non-retryable error");
}

#[tokio::test]
async fn no_fallback_when_none_configured() {
    // Primary model returns retryable error, but no fallback is configured.
    let primary_stream = Arc::new(MockStreamFn::new(vec![error_events(
        "rate limit exceeded 429",
    )]));

    let config = default_config(primary_stream, None);
    let stream = agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );

    let events = collect_events(stream).await;

    let has_fallback = events
        .iter()
        .any(|e| matches!(e, AgentEvent::ModelFallback { .. }));
    assert!(
        !has_fallback,
        "should not emit fallback event when none configured"
    );

    // Should still get an error result
    let has_error = events.iter().any(|e| {
        if let AgentEvent::MessageEnd { message } = e {
            message.stop_reason == StopReason::Error
        } else {
            false
        }
    });
    assert!(has_error, "should emit error when no fallback configured");
}

#[tokio::test]
async fn fallback_chain_tries_multiple_models() {
    // Primary fails, first fallback also fails, second fallback succeeds.
    let primary_stream = Arc::new(MockStreamFn::new(vec![error_events(
        "rate limit exceeded 429",
    )]));
    let fallback1_stream = Arc::new(MockStreamFn::new(vec![error_events(
        "rate limit exceeded 429",
    )]));
    let fallback2_stream = Arc::new(MockStreamFn::new(vec![text_only_events(
        "second fallback response",
    )]));

    let fb_model1 = ModelSpec::new("test", "fallback-1");
    let fb_model2 = ModelSpec::new("test", "fallback-2");

    let fallback = ModelFallback::new(vec![
        (fb_model1, fallback1_stream as Arc<dyn StreamFn>),
        (fb_model2, fallback2_stream as Arc<dyn StreamFn>),
    ]);

    let config = default_config(primary_stream, Some(fallback));
    let stream = agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );

    let events = collect_events(stream).await;

    // Should have two ModelFallback events (one per fallback attempt)
    let fallback_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ModelFallback { .. }))
        .count();
    assert_eq!(fallback_count, 2, "expected 2 ModelFallback events");

    // Should succeed from the second fallback
    let has_success = events.iter().any(|e| {
        if let AgentEvent::MessageEnd { message } = e {
            message.stop_reason == StopReason::Stop
        } else {
            false
        }
    });
    assert!(
        has_success,
        "expected successful response from second fallback"
    );
}

#[tokio::test]
async fn fallback_event_carries_model_info() {
    let primary_stream = Arc::new(MockStreamFn::new(vec![error_events(
        "rate limit exceeded 429",
    )]));
    let fallback_stream = Arc::new(MockStreamFn::new(vec![text_only_events("ok")]));

    let fb_model = fallback_model();
    let fallback = ModelFallback::new(vec![(
        fb_model.clone(),
        fallback_stream as Arc<dyn StreamFn>,
    )]);

    let config = default_config(primary_stream, Some(fallback));
    let stream = agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );

    let events = collect_events(stream).await;

    let fb_event = events.iter().find_map(|e| {
        if let AgentEvent::ModelFallback {
            from_model,
            to_model,
        } = e
        {
            Some((from_model.clone(), to_model.clone()))
        } else {
            None
        }
    });

    let (from, to) = fb_event.expect("expected ModelFallback event");
    assert_eq!(from.model_id, "primary-model");
    assert_eq!(to.model_id, "fallback-model");
}

#[tokio::test]
async fn all_fallbacks_exhausted_returns_error() {
    // Primary and all fallbacks fail with retryable errors.
    let primary_stream = Arc::new(MockStreamFn::new(vec![error_events(
        "rate limit exceeded 429",
    )]));
    let fallback_stream = Arc::new(MockStreamFn::new(vec![error_events(
        "rate limit exceeded 429",
    )]));

    let fallback = ModelFallback::new(vec![(
        fallback_model(),
        fallback_stream as Arc<dyn StreamFn>,
    )]);

    let config = default_config(primary_stream, Some(fallback));
    let stream = agent_loop(
        vec![user_msg("hello")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );

    let events = collect_events(stream).await;

    // Should have an error result when all models are exhausted
    let has_error = events.iter().any(|e| {
        if let AgentEvent::TurnEnd {
            assistant_message, ..
        } = e
        {
            assistant_message.stop_reason == StopReason::Error
        } else {
            false
        }
    });
    assert!(has_error, "expected error when all fallbacks exhausted");
}
