//! Tests for `provider`.
#![cfg(test)]

use super::*;
use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_sdk::trace::{SpanEvents, SpanLinks};
use std::borrow::Cow;
use std::time::{Duration, SystemTime};

fn make_span(name: &str, attrs: Vec<KeyValue>, complete: bool) -> SpanData {
    let start = SystemTime::now();
    let end = if complete {
        start + Duration::from_millis(1)
    } else {
        start
    };
    SpanData {
        span_context: SpanContext::new(
            TraceId::from(1_u128),
            SpanId::from(1_u64),
            TraceFlags::default(),
            false,
            TraceState::default(),
        ),
        parent_span_id: SpanId::INVALID,
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
        instrumentation_scope: InstrumentationScope::builder("test").build(),
    }
}

#[test]
fn raw_session_reports_session_id() {
    let s = RawSession::OtelSpans {
        session_id: "abc".into(),
        spans: vec![],
    };
    assert_eq!(s.session_id(), "abc");
}

#[test]
fn otel_spans_constructor_builds_the_otel_variant() {
    let span = make_span("root", vec![KeyValue::new("session.id", "S9")], true);
    let s = RawSession::otel_spans("S9", vec![span]);
    assert_eq!(s.session_id(), "S9");
    match s {
        RawSession::OtelSpans { session_id, spans } => {
            assert_eq!(session_id, "S9");
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].name, "root");
        }
    }
}

#[test]
fn trace_provider_error_display_includes_fields() {
    let err = TraceProviderError::SessionNotFound {
        session_id: "sid".into(),
    };
    assert!(format!("{err}").contains("sid"));
    let err = TraceProviderError::SessionInProgress {
        session_id: "sid".into(),
        open_spans: 2,
    };
    let rendered = format!("{err}");
    assert!(rendered.contains("sid"));
    assert!(rendered.contains('2'));
}

#[tokio::test]
async fn fetch_session_not_found_when_no_spans_match() {
    let exporter = InMemorySpanExporter::default();
    let provider = OtelInMemoryTraceProvider::new(exporter);
    let err = provider
        .fetch_session("missing")
        .await
        .expect_err("empty exporter");
    assert!(matches!(err, TraceProviderError::SessionNotFound { .. }));
}

#[tokio::test]
async fn fetch_session_uses_configured_attribute_key() {
    let exporter = InMemorySpanExporter::default();
    let provider =
        OtelInMemoryTraceProvider::new(exporter.clone()).with_session_attribute("custom.sid");
    assert_eq!(provider.session_attribute(), "custom.sid");

    // Writing into the exporter directly simulates a recorded session.
    use opentelemetry_sdk::trace::SpanExporter;
    let span = make_span("root", vec![KeyValue::new("custom.sid", "S1")], true);
    exporter.export(vec![span]).await.unwrap();

    let raw = provider.fetch_session("S1").await.unwrap();
    match raw {
        RawSession::OtelSpans { session_id, spans } => {
            assert_eq!(session_id, "S1");
            assert_eq!(spans.len(), 1);
        }
    }
}
