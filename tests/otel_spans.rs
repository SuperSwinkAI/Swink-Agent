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
use opentelemetry_sdk::trace::{InMemorySpanExporterBuilder, SdkTracerProvider};

use tokio_util::sync::CancellationToken;
use tracing_subscriber::prelude::*;

use common::{MockStreamFn, MockTool, default_model, text_only_events, tool_call_events};
use swink_agent::{
    AgentEvent, AgentLoopConfig, AgentMessage, ContentBlock, DefaultRetryStrategy, LlmMessage,
    StreamFn, StreamOptions, UserMessage, agent_loop,
};

type ConvertToLlmBoxed = Box<dyn Fn(&AgentMessage) -> Option<LlmMessage> + Send + Sync>;

fn default_convert_to_llm() -> ConvertToLlmBoxed {
    Box::new(swink_agent::default_convert)
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

#[tokio::test]
async fn otel_span_hierarchy() {
    let (exporter, _guard) = setup_otel_tracing();

    let stream_fn: Arc<dyn StreamFn> =
        Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let config = default_config(stream_fn);

    let stream = agent_loop(
        vec![user_msg("hi")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );
    let _events: Vec<AgentEvent> = stream.collect().await;

    // Small delay for the simple exporter to flush.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let spans = exporter.get_finished_spans().unwrap();
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
        turn_span.parent_span_id, run_span.span_context.span_id(),
        "agent.turn should be child of agent.run"
    );
    assert_eq!(
        llm_span.parent_span_id, turn_span.span_context.span_id(),
        "agent.llm_call should be child of agent.turn"
    );
}

#[tokio::test]
async fn otel_span_attributes() {
    let (exporter, _guard) = setup_otel_tracing();

    let stream_fn: Arc<dyn StreamFn> =
        Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let config = default_config(stream_fn);

    let stream = agent_loop(
        vec![user_msg("hi")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );
    let _events: Vec<AgentEvent> = stream.collect().await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let spans = exporter.get_finished_spans().unwrap();

    // Check agent.turn has turn_index attribute
    let turn_span = spans.iter().find(|s| s.name == "agent.turn").unwrap();
    let turn_attrs: Vec<(&opentelemetry::Key, &opentelemetry::Value)> =
        turn_span.attributes.iter().map(|kv| (&kv.key, &kv.value)).collect();
    assert!(
        turn_attrs.iter().any(|(k, _)| k.as_str() == "agent.turn_index"),
        "agent.turn should have agent.turn_index attribute, got: {turn_attrs:?}"
    );

    // Check agent.llm_call has model attribute
    let llm_span = spans.iter().find(|s| s.name == "agent.llm_call").unwrap();
    let llm_attrs: Vec<(&opentelemetry::Key, &opentelemetry::Value)> =
        llm_span.attributes.iter().map(|kv| (&kv.key, &kv.value)).collect();
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

    tokio::time::sleep(Duration::from_millis(50)).await;

    let spans = exporter.get_finished_spans().unwrap();
    let span_names: Vec<&str> = spans.iter().map(|s| s.name.as_ref()).collect();

    assert!(
        span_names.contains(&"agent.tool"),
        "expected agent.tool span, got: {span_names:?}"
    );

    // Verify tool span has agent.tool.name attribute
    let tool_span = spans.iter().find(|s| s.name == "agent.tool").unwrap();
    let tool_attrs: Vec<(&opentelemetry::Key, &opentelemetry::Value)> =
        tool_span.attributes.iter().map(|kv| (&kv.key, &kv.value)).collect();
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

    tokio::time::sleep(Duration::from_millis(50)).await;

    let spans = exporter.get_finished_spans().unwrap();

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
    use swink_agent::metrics::{MetricsCollector, TurnMetrics};

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
    let stream_fn: Arc<dyn StreamFn> =
        Arc::new(MockStreamFn::new(vec![text_only_events("hello")]));
    let mut config = default_config(stream_fn);
    config.metrics_collector = Some(collector.clone());

    let stream = agent_loop(
        vec![user_msg("hi")],
        "system".to_string(),
        config,
        CancellationToken::new(),
    );
    let _events: Vec<AgentEvent> = stream.collect().await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // OTel exporter received spans
    let spans = exporter.get_finished_spans().unwrap();
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
