//! Wiremock-backed tests for [`OtlpHttpTraceProvider`] (spec 043 T126).
//!
//! Coverage per the US6 per-backend-provider contract:
//!
//! * `otlp_happy_path_round_trip` — canned OTLP-JSON response maps
//!   through `OtelGenAiSessionMapper` into an `Invocation` with the
//!   expected provider/model/token counts (SC-008 shape).
//! * `otlp_missing_attribute_surfaces_as_mapping_error` — a span that
//!   omits `gen_ai.system` trips `MappingError::MissingAttribute` in
//!   the downstream mapper.
//! * `otlp_partial_session_surfaces_as_in_progress` — any span with
//!   `endTimeUnixNano == startTimeUnixNano` surfaces as
//!   `TraceProviderError::SessionInProgress` from the provider itself.
//! * `otlp_unknown_session_reports_not_found` — backend 404 and empty
//!   `resourceSpans` both surface as `SessionNotFound`.
//! * `otlp_backend_error_surfaces_as_backend_failure` — 500 status maps
//!   to `BackendFailure` without panicking.
//!
//! All tests use `wiremock::MockServer`; no live network (FR-050).

#![cfg(all(feature = "trace-ingest", feature = "trace-otlp"))]

use serde_json::json;
use swink_agent_eval::trace::{
    GenAIConventionVersion, MappingError, OpenInferenceSessionMapper, OtelGenAiSessionMapper,
    OtlpHttpTraceProvider, SessionMapper, TraceProvider, TraceProviderError,
};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn nanos(ms: u64) -> u64 {
    // Helper: milliseconds past epoch → OTLP unix-nanos (as JSON string,
    // per the OTLP-HTTP+JSON convention that integer ids/timestamps ride
    // over the wire as strings to survive JS number precision).
    ms.saturating_mul(1_000_000)
}

#[allow(clippy::needless_pass_by_value)]
fn otlp_span(
    session_id: &str,
    span_id_hex: &str,
    trace_id_hex: &str,
    start_ns: u64,
    end_ns: u64,
    extra_attrs: Vec<(&str, serde_json::Value)>,
) -> serde_json::Value {
    let mut attributes = vec![json!({
        "key": "session.id",
        "value": { "stringValue": session_id },
    })];
    for (k, v) in extra_attrs {
        attributes.push(json!({
            "key": k,
            "value": v,
        }));
    }
    json!({
        "traceId": trace_id_hex,
        "spanId": span_id_hex,
        "parentSpanId": "",
        "name": "llm.call",
        "kind": 1,
        "startTimeUnixNano": start_ns.to_string(),
        "endTimeUnixNano": end_ns.to_string(),
        "attributes": attributes,
    })
}

#[allow(clippy::needless_pass_by_value)]
fn wrap_spans(spans: Vec<serde_json::Value>) -> serde_json::Value {
    json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    { "key": "service.name", "value": { "stringValue": "eval-harness" } }
                ]
            },
            "scopeSpans": [{
                "scope": { "name": "swink-agent", "version": "0.8.1" },
                "spans": spans,
            }]
        }]
    })
}

#[tokio::test]
async fn otlp_happy_path_round_trip() {
    let server = MockServer::start().await;

    let span = otlp_span(
        "sess-1",
        "0000000000000001",
        "00000000000000000000000000000001",
        nanos(1_700_000_000_000),
        nanos(1_700_000_000_250),
        vec![
            ("gen_ai.system", json!({ "stringValue": "anthropic" })),
            ("gen_ai.request.model", json!({ "stringValue": "claude-3" })),
            ("gen_ai.usage.input_tokens", json!({ "intValue": "11" })),
            ("gen_ai.usage.output_tokens", json!({ "intValue": "22" })),
        ],
    );

    Mock::given(method("GET"))
        .and(path("/v1/traces"))
        .and(query_param("session.id", "sess-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(wrap_spans(vec![span])))
        .expect(1)
        .mount(&server)
        .await;

    let provider = OtlpHttpTraceProvider::new(server.uri()).expect("build provider");
    let raw = provider.fetch_session("sess-1").await.expect("session");
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
async fn otlp_missing_attribute_surfaces_as_mapping_error() {
    let server = MockServer::start().await;

    // OpenInference requires `llm.provider`; omit it deliberately so the
    // mapper surfaces `MissingAttribute`.
    let span = otlp_span(
        "sess-2",
        "0000000000000002",
        "00000000000000000000000000000002",
        nanos(1_700_000_001_000),
        nanos(1_700_000_001_500),
        vec![("llm.model_name", json!({ "stringValue": "claude-3" }))],
    );

    Mock::given(method("GET"))
        .and(path("/v1/traces"))
        .respond_with(ResponseTemplate::new(200).set_body_json(wrap_spans(vec![span])))
        .mount(&server)
        .await;

    let provider = OtlpHttpTraceProvider::new(server.uri()).expect("build provider");
    let raw = provider.fetch_session("sess-2").await.expect("session");

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
async fn otlp_partial_session_surfaces_as_in_progress() {
    let server = MockServer::start().await;

    let start = nanos(1_700_000_002_000);
    let complete = otlp_span(
        "sess-3",
        "0000000000000003",
        "00000000000000000000000000000003",
        start,
        start + 1_000_000,
        vec![],
    );
    // Same start/end nanos → the provider counts this as an "open" span.
    let open = otlp_span(
        "sess-3",
        "0000000000000004",
        "00000000000000000000000000000003",
        start + 2_000_000,
        start + 2_000_000,
        vec![],
    );

    Mock::given(method("GET"))
        .and(path("/v1/traces"))
        .respond_with(ResponseTemplate::new(200).set_body_json(wrap_spans(vec![complete, open])))
        .mount(&server)
        .await;

    let provider = OtlpHttpTraceProvider::new(server.uri()).expect("build provider");
    let err = provider
        .fetch_session("sess-3")
        .await
        .expect_err("still in progress");
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
async fn otlp_unknown_session_reports_not_found_on_empty_body() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/traces"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "resourceSpans": [] })))
        .mount(&server)
        .await;

    let provider = OtlpHttpTraceProvider::new(server.uri()).expect("build provider");
    let err = provider
        .fetch_session("missing")
        .await
        .expect_err("empty body");
    assert!(matches!(err, TraceProviderError::SessionNotFound { .. }));
}

#[tokio::test]
async fn otlp_unknown_session_reports_not_found_on_404() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/traces"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let provider = OtlpHttpTraceProvider::new(server.uri()).expect("build provider");
    let err = provider.fetch_session("missing").await.expect_err("404");
    assert!(matches!(err, TraceProviderError::SessionNotFound { .. }));
}

#[tokio::test]
async fn otlp_backend_error_surfaces_as_backend_failure() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/traces"))
        .respond_with(ResponseTemplate::new(500).set_body_string("tempo blew up"))
        .mount(&server)
        .await;

    let provider = OtlpHttpTraceProvider::new(server.uri()).expect("build provider");
    let err = provider.fetch_session("sess-5").await.expect_err("500");
    match err {
        TraceProviderError::BackendFailure { reason } => {
            assert!(
                reason.contains("500"),
                "expected 500 in reason, got {reason}"
            );
        }
        other => panic!("expected BackendFailure, got {other:?}"),
    }
}

#[tokio::test]
async fn otlp_bearer_token_is_sent_when_configured() {
    let server = MockServer::start().await;

    let span = otlp_span(
        "sess-auth",
        "0000000000000006",
        "00000000000000000000000000000006",
        nanos(1_700_000_003_000),
        nanos(1_700_000_003_100),
        vec![
            ("gen_ai.system", json!({ "stringValue": "openai" })),
            ("gen_ai.request.model", json!({ "stringValue": "gpt-4" })),
        ],
    );

    Mock::given(method("GET"))
        .and(path("/v1/traces"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer secret-token",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(wrap_spans(vec![span])))
        .expect(1)
        .mount(&server)
        .await;

    let provider = OtlpHttpTraceProvider::new(server.uri())
        .expect("build provider")
        .with_bearer_token("secret-token");
    let raw = provider
        .fetch_session("sess-auth")
        .await
        .expect("authed session");
    let inv = OtelGenAiSessionMapper::new(GenAIConventionVersion::V1_30)
        .map(&raw)
        .expect("map ok");
    assert_eq!(inv.model.provider, "openai");
}
