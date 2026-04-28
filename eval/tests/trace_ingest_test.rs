//! Integration tests for the `trace-ingest` core surface (spec 043 T119).
//!
//! Covers:
//! * `OtelInMemoryTraceProvider` round-trip (record + re-load).
//! * Missing required attribute → `MappingError::MissingAttribute`.
//! * Partially-written session (unfinished span) → `TraceProviderError::SessionInProgress`.

#![cfg(feature = "trace-ingest")]

use std::borrow::Cow;
use std::time::{Duration, SystemTime};

use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_sdk::trace::{
    InMemorySpanExporter, SpanData, SpanEvents, SpanExporter, SpanLinks,
};
use swink_agent_eval::trace::{
    GenAIConventionVersion, MappingError, OpenInferenceSessionMapper, OtelGenAiSessionMapper,
    OtelInMemoryTraceProvider, RawSession, SessionMapper, TraceProvider, TraceProviderError,
};

fn span_with(
    name: &str,
    attrs: Vec<KeyValue>,
    span_id: u64,
    parent: Option<u64>,
    complete: bool,
) -> SpanData {
    let start = SystemTime::now();
    let end = if complete {
        start + Duration::from_millis(2)
    } else {
        start // open span: end_time == start_time
    };
    SpanData {
        span_context: SpanContext::new(
            TraceId::from(42_u128),
            SpanId::from(span_id),
            TraceFlags::default(),
            false,
            TraceState::default(),
        ),
        parent_span_id: parent.map_or(SpanId::INVALID, SpanId::from),
        parent_span_is_remote: false,
        span_kind: SpanKind::Internal,
        name: Cow::Owned(name.to_string()),
        start_time: start,
        end_time: end,
        attributes: attrs,
        dropped_attributes_count: 0,
        events: SpanEvents::default(),
        links: SpanLinks::default(),
        status: Status::Unset,
        instrumentation_scope: InstrumentationScope::builder("integration-test").build(),
    }
}

#[tokio::test]
async fn otel_in_memory_trace_provider_round_trip() {
    let exporter = InMemorySpanExporter::default();
    let provider = OtelInMemoryTraceProvider::new(exporter.clone());

    let llm = span_with(
        "llm.call",
        vec![
            KeyValue::new("session.id", "sess-1"),
            KeyValue::new("gen_ai.system", "anthropic"),
            KeyValue::new("gen_ai.request.model", "claude-3"),
            KeyValue::new("gen_ai.usage.input_tokens", 11_i64),
            KeyValue::new("gen_ai.usage.output_tokens", 22_i64),
        ],
        1,
        None,
        true,
    );
    exporter.export(vec![llm]).await.expect("export ok");

    let raw = provider
        .fetch_session("sess-1")
        .await
        .expect("session found");
    assert_eq!(raw.session_id(), "sess-1");

    let inv = OtelGenAiSessionMapper::new(GenAIConventionVersion::V1_30)
        .map(&raw)
        .expect("map ok");
    assert_eq!(inv.model.provider, "anthropic");
    assert_eq!(inv.model.model_id, "claude-3");
    assert_eq!(inv.total_usage.input, 11);
    assert_eq!(inv.total_usage.output, 22);
    assert_eq!(inv.total_usage.total, 33);
}

#[tokio::test]
async fn missing_attribute_surfaces_as_mapping_error() {
    let exporter = InMemorySpanExporter::default();
    let provider = OtelInMemoryTraceProvider::new(exporter.clone());

    // OpenInference requires `llm.provider`; omit it deliberately.
    let llm = span_with(
        "llm.call",
        vec![
            KeyValue::new("session.id", "sess-2"),
            KeyValue::new("llm.model_name", "claude-3"),
        ],
        1,
        None,
        true,
    );
    exporter.export(vec![llm]).await.unwrap();

    let raw = provider.fetch_session("sess-2").await.unwrap();
    let err = OpenInferenceSessionMapper
        .map(&raw)
        .expect_err("provider attribute missing");
    match err {
        MappingError::MissingAttribute { name } => {
            assert_eq!(name, OpenInferenceSessionMapper::PROVIDER_KEY);
        }
        other => panic!("expected MissingAttribute, got {other:?}"),
    }
}

#[tokio::test]
async fn partial_session_surfaces_as_session_in_progress() {
    let exporter = InMemorySpanExporter::default();
    let provider = OtelInMemoryTraceProvider::new(exporter.clone());

    // One complete + one open span → provider MUST refuse.
    let complete = span_with(
        "llm.call",
        vec![KeyValue::new("session.id", "sess-3")],
        1,
        None,
        true,
    );
    let open = span_with(
        "tool.call",
        vec![KeyValue::new("session.id", "sess-3")],
        2,
        Some(1),
        false,
    );
    exporter.export(vec![complete, open]).await.unwrap();

    let err = provider
        .fetch_session("sess-3")
        .await
        .expect_err("session still in progress");
    match err {
        TraceProviderError::SessionInProgress {
            session_id,
            open_spans,
        } => {
            assert_eq!(session_id, "sess-3");
            assert!(open_spans >= 1, "at least one open span reported");
        }
        other => panic!("expected SessionInProgress, got {other:?}"),
    }
}

#[tokio::test]
async fn unknown_session_reports_not_found() {
    let exporter = InMemorySpanExporter::default();
    let provider = OtelInMemoryTraceProvider::new(exporter);
    let err = provider.fetch_session("absent").await.expect_err("empty");
    assert!(matches!(err, TraceProviderError::SessionNotFound { .. }));
}

#[test]
fn raw_session_otel_variant_exposes_session_id() {
    let raw = RawSession::OtelSpans {
        session_id: "xyz".into(),
        spans: vec![],
    };
    assert_eq!(raw.session_id(), "xyz");
}
