//! Wiremock-backed tests for [`LangfuseTraceProvider`] (spec 043 T127).
//!
//! Coverage per the US6 per-backend-provider contract:
//!
//! * `langfuse_happy_path_round_trip` — canned Langfuse trace response
//!   with GenAI metadata maps through `OtelGenAiSessionMapper` into an
//!   `Invocation` with the expected provider/model/token counts.
//! * `langfuse_missing_attribute_surfaces_as_mapping_error` — a trace
//!   whose metadata omits `gen_ai.system` trips
//!   `MappingError::MissingAttribute` downstream.
//! * `langfuse_partial_session_surfaces_as_in_progress` — any
//!   observation with `endTime: null` surfaces as `SessionInProgress`
//!   from the provider itself.
//! * `langfuse_unknown_session_reports_not_found` — backend 404 surfaces
//!   as `SessionNotFound`.
//! * `langfuse_backend_error_surfaces_as_backend_failure` — 500 status
//!   maps to `BackendFailure` without panicking.
//!
//! All tests use `wiremock::MockServer`; no live network (FR-050).

#![cfg(all(feature = "trace-ingest", feature = "trace-langfuse"))]

use serde_json::json;
use swink_agent_eval::trace::{
    GenAIConventionVersion, LangfuseTraceProvider, MappingError, OpenInferenceSessionMapper,
    OtelGenAiSessionMapper, SessionMapper, TraceProvider, TraceProviderError,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[allow(clippy::needless_pass_by_value)]
fn observation(
    id: &str,
    obs_type: &str,
    start: &str,
    end: Option<&str>,
    metadata: serde_json::Value,
) -> serde_json::Value {
    let end_val = match end {
        Some(e) => json!(e),
        None => serde_json::Value::Null,
    };
    json!({
        "id": id,
        "parentObservationId": null,
        "name": "llm.call",
        "type": obs_type,
        "startTime": start,
        "endTime": end_val,
        "metadata": metadata,
    })
}

#[allow(clippy::needless_pass_by_value)]
fn trace_body(trace_id: &str, observations: Vec<serde_json::Value>) -> serde_json::Value {
    json!({
        "id": trace_id,
        "name": "offline-eval-session",
        "observations": observations,
    })
}

#[tokio::test]
async fn langfuse_happy_path_round_trip() {
    let server = MockServer::start().await;

    let obs = observation(
        "obs-1",
        "GENERATION",
        "2026-04-23T10:00:00Z",
        Some("2026-04-23T10:00:01Z"),
        json!({
            "gen_ai.system": "anthropic",
            "gen_ai.request.model": "claude-3",
            "gen_ai.usage.input_tokens": 11,
            "gen_ai.usage.output_tokens": 22,
        }),
    );

    Mock::given(method("GET"))
        .and(path("/api/public/traces/trace-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(trace_body("trace-1", vec![obs])))
        .expect(1)
        .mount(&server)
        .await;

    let provider =
        LangfuseTraceProvider::new(server.uri(), "pk-test", "sk-test").expect("build provider");
    let raw = provider
        .fetch_session("trace-1")
        .await
        .expect("session found");
    assert_eq!(raw.session_id(), "trace-1");

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
async fn langfuse_missing_attribute_surfaces_as_mapping_error() {
    let server = MockServer::start().await;

    // Metadata omits `llm.provider` — OpenInference mapper requires it.
    let obs = observation(
        "obs-2",
        "GENERATION",
        "2026-04-23T10:00:00Z",
        Some("2026-04-23T10:00:01Z"),
        json!({
            "llm.model_name": "claude-3",
        }),
    );

    Mock::given(method("GET"))
        .and(path("/api/public/traces/trace-2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(trace_body("trace-2", vec![obs])))
        .mount(&server)
        .await;

    let provider = LangfuseTraceProvider::new(server.uri(), "pk", "sk").expect("build provider");
    let raw = provider
        .fetch_session("trace-2")
        .await
        .expect("session found");

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
async fn langfuse_partial_session_surfaces_as_in_progress() {
    let server = MockServer::start().await;

    let complete = observation(
        "obs-3a",
        "SPAN",
        "2026-04-23T10:00:00Z",
        Some("2026-04-23T10:00:01Z"),
        json!({}),
    );
    // `endTime: null` is how Langfuse signals an in-flight observation.
    let open = observation("obs-3b", "SPAN", "2026-04-23T10:00:01Z", None, json!({}));

    Mock::given(method("GET"))
        .and(path("/api/public/traces/trace-3"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(trace_body("trace-3", vec![complete, open])),
        )
        .mount(&server)
        .await;

    let provider = LangfuseTraceProvider::new(server.uri(), "pk", "sk").expect("build provider");
    let err = provider
        .fetch_session("trace-3")
        .await
        .expect_err("still in progress");
    match err {
        TraceProviderError::SessionInProgress {
            session_id,
            open_spans,
        } => {
            assert_eq!(session_id, "trace-3");
            assert!(open_spans >= 1, "at least one open observation reported");
        }
        other => panic!("expected SessionInProgress, got {other:?}"),
    }
}

#[tokio::test]
async fn langfuse_unknown_session_reports_not_found_on_404() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/public/traces/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let provider = LangfuseTraceProvider::new(server.uri(), "pk", "sk").expect("build provider");
    let err = provider.fetch_session("missing").await.expect_err("404");
    assert!(matches!(err, TraceProviderError::SessionNotFound { .. }));
}

#[tokio::test]
async fn langfuse_unknown_session_reports_not_found_on_empty_observations() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/public/traces/empty"))
        .respond_with(ResponseTemplate::new(200).set_body_json(trace_body("empty", Vec::new())))
        .mount(&server)
        .await;

    let provider = LangfuseTraceProvider::new(server.uri(), "pk", "sk").expect("build provider");
    let err = provider
        .fetch_session("empty")
        .await
        .expect_err("empty observations");
    assert!(matches!(err, TraceProviderError::SessionNotFound { .. }));
}

#[tokio::test]
async fn langfuse_backend_error_surfaces_as_backend_failure() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/public/traces/boom"))
        .respond_with(ResponseTemplate::new(500).set_body_string("langfuse blew up"))
        .mount(&server)
        .await;

    let provider = LangfuseTraceProvider::new(server.uri(), "pk", "sk").expect("build provider");
    let err = provider.fetch_session("boom").await.expect_err("500");
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
async fn langfuse_basic_auth_header_is_sent() {
    let server = MockServer::start().await;

    // HTTP Basic: base64("pk-demo:sk-demo") = cGstZGVtbzpzay1kZW1v
    let obs = observation(
        "obs-auth",
        "GENERATION",
        "2026-04-23T10:00:00Z",
        Some("2026-04-23T10:00:01Z"),
        json!({
            "gen_ai.system": "openai",
            "gen_ai.request.model": "gpt-4",
        }),
    );
    Mock::given(method("GET"))
        .and(path("/api/public/traces/auth-check"))
        .and(wiremock::matchers::header(
            "authorization",
            "Basic cGstZGVtbzpzay1kZW1v",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(trace_body("auth-check", vec![obs])))
        .expect(1)
        .mount(&server)
        .await;

    let provider =
        LangfuseTraceProvider::new(server.uri(), "pk-demo", "sk-demo").expect("build provider");
    let raw = provider
        .fetch_session("auth-check")
        .await
        .expect("authed session");
    let inv = OtelGenAiSessionMapper::new(GenAIConventionVersion::V1_30)
        .map(&raw)
        .expect("map ok");
    assert_eq!(inv.model.provider, "openai");
}
