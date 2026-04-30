#![cfg(feature = "testkit")]
//! Integration tests for OpenTelemetry span instrumentation.
//!
//! These tests verify that the agent loop emits properly hierarchical `OTel` spans
//! with semantic attributes. They use an in-memory exporter so no collector is
//! needed.

mod common;

use std::sync::Arc;
use std::time::Duration;

use futures::stream::StreamExt;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::{InMemorySpanExporterBuilder, SdkTracerProvider, SpanData};

use tokio_util::sync::CancellationToken;
use tracing_subscriber::prelude::*;

use common::{MockStreamFn, MockTool, default_model, text_only_events, tool_call_events};
use opentelemetry::trace::Status as OtelStatus;
use swink_agent::{
    AgentEvent, AgentLoopConfig, AgentMessage, AssistantMessageEvent, ContentBlock,
    DefaultRetryStrategy, LlmMessage, ModelFallback, ModelSpec, StopReason, StreamFn,
    StreamOptions, UserMessage, agent_loop,
};

type ConvertToLlmBoxed = Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync>;

fn default_convert_to_llm() -> ConvertToLlmBoxed {
    Box::new(swink_agent::default_convert)
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
        session_state: Arc::new(std::sync::RwLock::new(swink_agent::SessionState::new())),
        credential_resolver: None,
        cache_config: None,
        cache_state: std::sync::Mutex::new(swink_agent::CacheState::default()),
        dynamic_system_prompt: None,
    }
}

fn user_msg(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        timestamp: swink_agent::now_timestamp(),
        cache_hint: None,
    }))
}

/// Set up an in-memory `OTel` exporter and tracing subscriber, returning the
/// exporter handle for span inspection after the test.
fn setup_otel_tracing() -> (
    opentelemetry_sdk::trace::InMemorySpanExporter,
    tracing::subscriber::DefaultGuard,
) {
    let exporter = InMemorySpanExporterBuilder::new().build();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let tracer = provider.tracer("test");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);
    let subscriber = tracing_subscriber::registry().with(otel_layer);
    let guard = tracing::subscriber::set_default(subscriber);
    (exporter, guard)
}

async fn collect_finished_spans_until<F>(
    exporter: &opentelemetry_sdk::trace::InMemorySpanExporter,
    is_ready: F,
) -> Vec<SpanData>
where
    F: Fn(&[SpanData]) -> bool,
{
    let mut spans = Vec::new();

    for _ in 0..16 {
        spans = exporter.get_finished_spans().unwrap();
        if is_ready(&spans) {
            return spans;
        }
        tokio::task::yield_now().await;
    }

    spans
}

#[tokio::test]
async fn otel_span_hierarchy() {
    let (exporter, _guard) = setup_otel_tracing();

    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let config = default_config(stream_fn);

    let stream = agent_loop(
        vec![user_msg("hi")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );
    let _events: Vec<AgentEvent> = stream.collect().await;

    let spans = collect_finished_spans_until(&exporter, |spans| {
        spans.iter().any(|s| s.name == "agent.run")
            && spans.iter().any(|s| s.name == "agent.turn")
            && spans.iter().any(|s| s.name == "agent.llm_call")
    })
    .await;
    let span_names: Vec<&str> = spans.iter().map(|s| s.name.as_ref()).collect();

    assert!(
        span_names.contains(&"agent.run"),
        "expected agent.run span, got: {span_names:?}"
    );
    assert!(
        span_names.contains(&"agent.turn"),
        "expected agent.turn span, got: {span_names:?}"
    );
    assert!(
        span_names.contains(&"agent.llm_call"),
        "expected agent.llm_call span, got: {span_names:?}"
    );

    // Verify parent-child: agent.turn should be child of agent.run
    let run_span = spans.iter().find(|s| s.name == "agent.run").unwrap();
    let turn_span = spans.iter().find(|s| s.name == "agent.turn").unwrap();
    let llm_span = spans.iter().find(|s| s.name == "agent.llm_call").unwrap();

    assert_eq!(
        turn_span.parent_span_id,
        run_span.span_context.span_id(),
        "agent.turn should be child of agent.run"
    );
    assert_eq!(
        llm_span.parent_span_id,
        turn_span.span_context.span_id(),
        "agent.llm_call should be child of agent.turn"
    );
}

#[tokio::test]
async fn otel_span_attributes() {
    let (exporter, _guard) = setup_otel_tracing();

    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let config = default_config(stream_fn);

    let stream = agent_loop(
        vec![user_msg("hi")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );
    let _events: Vec<AgentEvent> = stream.collect().await;

    let spans = collect_finished_spans_until(&exporter, |spans| {
        spans.iter().any(|s| s.name == "agent.turn")
            && spans.iter().any(|s| s.name == "agent.llm_call")
    })
    .await;

    // Check agent.turn has turn_index attribute
    let turn_span = spans.iter().find(|s| s.name == "agent.turn").unwrap();
    let turn_attrs: Vec<(&opentelemetry::Key, &opentelemetry::Value)> = turn_span
        .attributes
        .iter()
        .map(|kv| (&kv.key, &kv.value))
        .collect();
    assert!(
        turn_attrs
            .iter()
            .any(|(k, _)| k.as_str() == "agent.turn_index"),
        "agent.turn should have agent.turn_index attribute, got: {turn_attrs:?}"
    );

    // Check agent.llm_call has model attribute
    let llm_span = spans.iter().find(|s| s.name == "agent.llm_call").unwrap();
    let llm_attrs: Vec<(&opentelemetry::Key, &opentelemetry::Value)> = llm_span
        .attributes
        .iter()
        .map(|kv| (&kv.key, &kv.value))
        .collect();
    assert!(
        llm_attrs.iter().any(|(k, _)| k.as_str() == "agent.model"),
        "agent.llm_call should have agent.model attribute, got: {llm_attrs:?}"
    );
}

#[tokio::test]
async fn otel_tool_spans() {
    let (exporter, _guard) = setup_otel_tracing();

    let tool = MockTool::new("test_tool");
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "test_tool", "{}"),
        text_only_events("done"),
    ]));
    let mut config = default_config(stream_fn);
    config.tools = vec![Arc::new(tool)];

    let stream = agent_loop(
        vec![user_msg("use test_tool")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );
    let _events: Vec<AgentEvent> = stream.collect().await;

    let spans = collect_finished_spans_until(&exporter, |spans| {
        spans.iter().any(|s| s.name == "agent.tool")
    })
    .await;
    let span_names: Vec<&str> = spans.iter().map(|s| s.name.as_ref()).collect();

    assert!(
        span_names.contains(&"agent.tool"),
        "expected agent.tool span, got: {span_names:?}"
    );

    // Verify tool span has agent.tool.name attribute
    let tool_span = spans.iter().find(|s| s.name == "agent.tool").unwrap();
    let tool_attrs: Vec<(&opentelemetry::Key, &opentelemetry::Value)> = tool_span
        .attributes
        .iter()
        .map(|kv| (&kv.key, &kv.value))
        .collect();
    assert!(
        tool_attrs
            .iter()
            .any(|(k, _)| k.as_str() == "agent.tool.name"),
        "agent.tool should have agent.tool.name attribute, got: {tool_attrs:?}"
    );
}

#[tokio::test]
async fn otel_spans_exclude_content() {
    let (exporter, _guard) = setup_otel_tracing();

    let tool = MockTool::new("secret_tool");
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![
        tool_call_events("call_1", "secret_tool", r#"{"password": "s3cret"}"#),
        text_only_events("done"),
    ]));
    let mut config = default_config(stream_fn);
    config.tools = vec![Arc::new(tool)];

    let stream = agent_loop(
        vec![user_msg("use secret_tool")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );
    let _events: Vec<AgentEvent> = stream.collect().await;

    let spans = collect_finished_spans_until(&exporter, |spans| !spans.is_empty()).await;

    // Verify no span contains prompt text, tool arguments, or tool results
    for span in &spans {
        for kv in &span.attributes {
            let val_str = format!("{:?}", kv.value);
            assert!(
                !val_str.contains("s3cret"),
                "span {} attribute {} leaks content: {val_str}",
                span.name,
                kv.key
            );
            assert!(
                !val_str.contains("password"),
                "span {} attribute {} leaks argument names: {val_str}",
                span.name,
                kv.key
            );
        }
    }
}

#[tokio::test]
async fn otel_coexists_with_metrics_collector() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use swink_agent::{MetricsCollector, TurnMetrics};

    struct FlagCollector(AtomicBool);

    impl MetricsCollector for FlagCollector {
        fn on_metrics<'a>(
            &'a self,
            _metrics: &'a TurnMetrics,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            self.0.store(true, Ordering::SeqCst);
            Box::pin(async {})
        }
    }

    let (exporter, _guard) = setup_otel_tracing();

    let collector = Arc::new(FlagCollector(AtomicBool::new(false)));
    let stream_fn: Arc<dyn StreamFn> = Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let mut config = default_config(stream_fn);
    config.metrics_collector = Some(collector.clone());

    let stream = agent_loop(
        vec![user_msg("hi")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );
    let _events: Vec<AgentEvent> = stream.collect().await;

    // OTel exporter received spans
    let spans = collect_finished_spans_until(&exporter, |spans| !spans.is_empty()).await;
    assert!(
        !spans.is_empty(),
        "OTel exporter should have received spans"
    );

    // MetricsCollector also fired
    assert!(
        collector.0.load(Ordering::SeqCst),
        "MetricsCollector should also have received metrics"
    );
}

#[tokio::test]
async fn otel_model_fallback_spans() {
    let (exporter, _guard) = setup_otel_tracing();

    // Primary model returns a retryable error (rate limit).
    let primary_stream = Arc::new(MockStreamFn::new(vec![vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Error,
            error_message: "rate limit exceeded 429".to_string(),
            error_kind: None,
            usage: None,
        },
    ]]));

    // Fallback model succeeds.
    let fallback_model = ModelSpec::new("test", "fallback-model");
    let fallback_stream: Arc<dyn StreamFn> =
        Arc::new(MockStreamFn::new(vec![text_only_events("ok")]));

    let fallback = ModelFallback::new(vec![(fallback_model.clone(), fallback_stream)]);

    let mut config = default_config(primary_stream);
    // Exhaust retries immediately so fallback triggers.
    config.retry_strategy = Box::new(
        DefaultRetryStrategy::default()
            .with_max_attempts(1)
            .with_jitter(false)
            .with_base_delay(Duration::from_millis(1)),
    );
    config.fallback = Some(fallback);

    let stream = agent_loop(
        vec![user_msg("hi")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );
    let _events: Vec<AgentEvent> = stream.collect().await;

    let spans = collect_finished_spans_until(&exporter, |spans| {
        spans.iter().filter(|s| s.name == "agent.llm_call").count() == 2
    })
    .await;

    // Should have two agent.llm_call spans — one for primary (failed) and one for fallback.
    let llm_spans: Vec<_> = spans
        .iter()
        .filter(|s| s.name == "agent.llm_call")
        .collect();
    assert_eq!(
        llm_spans.len(),
        2,
        "expected 2 agent.llm_call spans (primary + fallback), got {}: {:?}",
        llm_spans.len(),
        llm_spans.iter().map(|s| &s.name).collect::<Vec<_>>()
    );

    // Both should be children of the same agent.turn span.
    let turn_span = spans
        .iter()
        .find(|s| s.name == "agent.turn")
        .expect("expected agent.turn span");
    for llm_span in &llm_spans {
        assert_eq!(
            llm_span.parent_span_id,
            turn_span.span_context.span_id(),
            "agent.llm_call span for model {:?} should be child of agent.turn",
            llm_span
                .attributes
                .iter()
                .find(|kv| kv.key.as_str() == "agent.model")
                .map(|kv| format!("{:?}", kv.value))
                .unwrap_or_default()
        );
    }

    // The failed primary span should have error status.
    let primary_span = llm_spans
        .iter()
        .find(|s| {
            s.attributes.iter().any(|kv| {
                kv.key.as_str() == "agent.model" && format!("{:?}", kv.value).contains("test-model")
            })
        })
        .expect("expected primary model span");
    assert!(
        matches!(primary_span.status, OtelStatus::Error { .. }),
        "primary model span should have error status, got: {:?}",
        primary_span.status
    );

    // The fallback span should have the fallback model name.
    let fallback_span = llm_spans
        .iter()
        .find(|s| {
            s.attributes.iter().any(|kv| {
                kv.key.as_str() == "agent.model"
                    && format!("{:?}", kv.value).contains("fallback-model")
            })
        })
        .expect("expected fallback model span");
    // Fallback span should NOT have error status.
    assert!(
        !matches!(fallback_span.status, OtelStatus::Error { .. }),
        "fallback model span should not have error status"
    );
}
