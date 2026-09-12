//! Wiremock-based tests for `ResponsesStreamFn` (issue #1261).

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use swink_agent::{
    AgentContext, AgentTool, AgentToolResult, AssistantMessageEvent, ModelSpec, StopReason,
    StreamErrorKind, StreamFn, StreamOptions, ThinkingLevel,
};
use swink_agent_adapters::{HeaderName, HeaderValue, ResponsesStreamFn};

use crate::common::{event_name, find_error_kind, find_error_message, sse_response, test_context};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn test_model() -> ModelSpec {
    ModelSpec::new("openai", "gpt-5.6-luna")
}

/// Build an `event:`/`data:` SSE body from (type, json) pairs.
fn sse(events: &[(&str, serde_json::Value)]) -> String {
    let mut out = String::new();
    for (event_type, data) in events {
        out.push_str(&format!("event: {event_type}\ndata: {data}\n\n"));
    }
    out
}

#[allow(clippy::needless_pass_by_value)] // call sites pass `json!` temporaries
fn completed(usage: serde_json::Value) -> (&'static str, serde_json::Value) {
    (
        "response.completed",
        serde_json::json!({"type": "response.completed", "response": {"id": "resp_1", "status": "completed", "usage": usage}}),
    )
}

async fn collect(
    stream_fn: &ResponsesStreamFn,
    context: &AgentContext,
    options: StreamOptions,
) -> Vec<AssistantMessageEvent> {
    let model = test_model();
    stream_fn
        .stream(&model, context, &options, CancellationToken::new())
        .collect::<Vec<_>>()
        .await
}

fn names(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
    events.iter().map(event_name).collect()
}

async fn last_request_body(server: &MockServer) -> serde_json::Value {
    let requests = server.received_requests().await.unwrap();
    serde_json::from_slice(&requests.last().unwrap().body).unwrap()
}

struct WeatherTool;

static WEATHER_SCHEMA: std::sync::LazyLock<serde_json::Value> = std::sync::LazyLock::new(
    || serde_json::json!({"type": "object", "properties": {"city": {"type": "string"}}}),
);

impl AgentTool for WeatherTool {
    fn name(&self) -> &'static str {
        "weather"
    }
    fn label(&self) -> &'static str {
        "Weather"
    }
    fn description(&self) -> &'static str {
        "Look up the weather"
    }
    fn parameters_schema(&self) -> &serde_json::Value {
        &WEATHER_SCHEMA
    }
    fn execute(
        &self,
        _tool_call_id: &str,
        _arguments: serde_json::Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::ResolvedCredential>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async { unreachable!("never executed in these tests") })
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn text_stream_round_trips_and_request_shape_is_responses() {
    let body = sse(&[
        (
            "response.created",
            serde_json::json!({"type": "response.created", "response": {"id": "resp_1"}}),
        ),
        (
            "response.output_item.added",
            serde_json::json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "msg_1", "role": "assistant"}}),
        ),
        (
            "response.output_text.delta",
            serde_json::json!({"type": "response.output_text.delta", "output_index": 0, "delta": "Hel"}),
        ),
        (
            "response.output_text.delta",
            serde_json::json!({"type": "response.output_text.delta", "output_index": 0, "delta": "lo"}),
        ),
        (
            "response.output_item.done",
            serde_json::json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_1"}}),
        ),
        completed(serde_json::json!({"input_tokens": 10, "output_tokens": 2, "total_tokens": 12})),
    ]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(sse_response(&body))
        .expect(1)
        .mount(&server)
        .await;

    let stream_fn = ResponsesStreamFn::new(server.uri(), "test-key");
    let events = collect(&stream_fn, &test_context(), StreamOptions::default()).await;
    assert_eq!(
        names(&events),
        [
            "Start",
            "TextStart",
            "TextDelta",
            "TextDelta",
            "TextEnd",
            "Done"
        ],
        "{events:?}"
    );
    match &events[5] {
        AssistantMessageEvent::Done {
            stop_reason, usage, ..
        } => {
            assert_eq!(*stop_reason, StopReason::Stop);
            assert_eq!(usage.input, 10);
            assert_eq!(usage.output, 2);
            assert_eq!(usage.total, 12);
        }
        other => panic!("expected Done, got {other:?}"),
    }

    let request = last_request_body(&server).await;
    assert_eq!(
        request["store"], false,
        "Codex backend requires store:false"
    );
    assert_eq!(request["stream"], true);
    assert!(
        !request["instructions"].as_str().unwrap().is_empty(),
        "instructions must be non-empty"
    );
    assert_eq!(request["model"], "gpt-5.6-luna");
    assert!(
        request.get("messages").is_none(),
        "Responses uses `input`, not `messages`"
    );
    assert!(request["input"].is_array());
}

#[tokio::test]
async fn tool_call_streams_and_tool_schema_is_flat() {
    let body = sse(&[
        (
            "response.output_item.added",
            serde_json::json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_abc", "name": "weather", "arguments": ""}}),
        ),
        (
            "response.function_call_arguments.delta",
            serde_json::json!({"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "{\"city\":"}),
        ),
        (
            "response.function_call_arguments.delta",
            serde_json::json!({"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "\"Oslo\"}"}),
        ),
        (
            "response.function_call_arguments.done",
            serde_json::json!({"type": "response.function_call_arguments.done", "output_index": 0, "arguments": "{\"city\":\"Oslo\"}"}),
        ),
        (
            "response.output_item.done",
            serde_json::json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "function_call", "id": "fc_1", "call_id": "call_abc", "name": "weather", "arguments": "{\"city\":\"Oslo\"}"}}),
        ),
        completed(serde_json::json!({"input_tokens": 20, "output_tokens": 8})),
    ]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;

    let context = AgentContext::new(
        "sys",
        Vec::new(),
        vec![Arc::new(WeatherTool) as Arc<dyn AgentTool>],
    );
    let stream_fn = ResponsesStreamFn::new(server.uri(), "test-key");
    let events = collect(&stream_fn, &context, StreamOptions::default()).await;
    assert_eq!(
        names(&events),
        [
            "Start",
            "ToolCallStart",
            "ToolCallDelta",
            "ToolCallDelta",
            "ToolCallEnd",
            "Done"
        ],
        "{events:?}"
    );
    match &events[1] {
        AssistantMessageEvent::ToolCallStart { id, name, .. } => {
            assert_eq!(
                id, "call_abc",
                "call_id, not the item id, is what the tool result must echo"
            );
            assert_eq!(name, "weather");
        }
        other => panic!("{other:?}"),
    }
    let args: String = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::ToolCallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        args, r#"{"city":"Oslo"}"#,
        "`arguments.done` must not re-emit already-streamed deltas"
    );
    assert!(matches!(
        &events[5],
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            ..
        }
    ));

    // The porting bug the issue warns about: Responses tools are flat.
    let request = last_request_body(&server).await;
    let tool = &request["tools"][0];
    assert_eq!(tool["type"], "function");
    assert_eq!(tool["name"], "weather");
    assert_eq!(tool["description"], "Look up the weather");
    assert_eq!(tool["parameters"]["type"], "object");
    assert!(
        tool.get("function").is_none(),
        "tool must not be nested under `function`: {tool}"
    );
    assert_eq!(request["tool_choice"], "auto");
}

#[tokio::test]
async fn tool_call_arriving_only_in_done_still_streams_arguments() {
    let body = sse(&[
        (
            "response.output_item.done",
            serde_json::json!({"type": "response.output_item.done", "output_index": 0, "item": {"type": "function_call", "call_id": "call_x", "name": "weather", "arguments": "{\"city\":\"Rome\"}"}}),
        ),
        completed(serde_json::json!({"input_tokens": 1, "output_tokens": 1})),
    ]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;
    let events = collect(
        &ResponsesStreamFn::new(server.uri(), "k"),
        &test_context(),
        StreamOptions::default(),
    )
    .await;
    assert_eq!(
        names(&events),
        [
            "Start",
            "ToolCallStart",
            "ToolCallDelta",
            "ToolCallEnd",
            "Done"
        ],
        "{events:?}"
    );
}

#[tokio::test]
async fn usage_accounting_splits_cached_and_keeps_reasoning_tokens() {
    let body = sse(&[
        (
            "response.output_text.delta",
            serde_json::json!({"type": "response.output_text.delta", "delta": "ok"}),
        ),
        completed(serde_json::json!({
            "input_tokens": 1000, "output_tokens": 50, "total_tokens": 1050,
            "input_tokens_details": {"cached_tokens": 600},
            "output_tokens_details": {"reasoning_tokens": 20}
        })),
    ]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;
    let events = collect(
        &ResponsesStreamFn::new(server.uri(), "k"),
        &test_context(),
        StreamOptions::default(),
    )
    .await;
    let usage = match events.last().unwrap() {
        AssistantMessageEvent::Done { usage, .. } => usage.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(usage.input, 400, "fresh input excludes cached tokens");
    assert_eq!(usage.cache_read, 600);
    assert_eq!(usage.output, 50);
    assert_eq!(usage.total, 1050);
    assert_eq!(usage.extra["output_tokens_details.reasoning_tokens"], 20);
}

#[tokio::test]
async fn reasoning_deltas_become_thinking_blocks_then_text() {
    let body = sse(&[
        (
            "response.reasoning_summary_text.delta",
            serde_json::json!({"type": "response.reasoning_summary_text.delta", "delta": "hmm"}),
        ),
        (
            "response.output_text.delta",
            serde_json::json!({"type": "response.output_text.delta", "delta": "answer"}),
        ),
        completed(serde_json::json!({"input_tokens": 1, "output_tokens": 1})),
    ]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;
    let model = test_model().with_thinking_level(ThinkingLevel::High);
    let stream_fn = ResponsesStreamFn::new(server.uri(), "k");
    let events = stream_fn
        .stream(
            &model,
            &test_context(),
            &StreamOptions::default(),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        names(&events),
        [
            "Start",
            "ThinkingStart",
            "ThinkingDelta",
            "ThinkingEnd",
            "TextStart",
            "TextDelta",
            "TextEnd",
            "Done"
        ],
        "{events:?}"
    );
    assert_eq!(
        last_request_body(&server).await["reasoning"]["effort"],
        "high"
    );
}

#[tokio::test]
async fn mid_stream_error_event_closes_open_blocks_then_errors() {
    let body = sse(&[
        (
            "response.output_text.delta",
            serde_json::json!({"type": "response.output_text.delta", "delta": "partial"}),
        ),
        (
            "error",
            serde_json::json!({"type": "error", "code": "server_error", "message": "boom"}),
        ),
    ]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;
    let events = collect(
        &ResponsesStreamFn::new(server.uri(), "k"),
        &test_context(),
        StreamOptions::default(),
    )
    .await;
    assert_eq!(
        names(&events),
        ["Start", "TextStart", "TextDelta", "TextEnd", "Error"],
        "{events:?}"
    );
    let message = find_error_message(&events).unwrap();
    assert!(
        message.contains("server_error") && message.contains("boom"),
        "{message}"
    );
    assert_eq!(
        find_error_kind(&events),
        Some(Some(StreamErrorKind::Network))
    );
}

#[tokio::test]
async fn rate_limit_error_event_is_throttled() {
    let body = sse(&[(
        "error",
        serde_json::json!({"type": "error", "code": "rate_limit_exceeded", "message": "slow down"}),
    )]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;
    let events = collect(
        &ResponsesStreamFn::new(server.uri(), "k"),
        &test_context(),
        StreamOptions::default(),
    )
    .await;
    assert_eq!(
        find_error_kind(&events),
        Some(Some(StreamErrorKind::Throttled))
    );
}

#[tokio::test]
async fn response_failed_context_length_is_context_overflow() {
    let body = sse(&[(
        "response.failed",
        serde_json::json!({
            "type": "response.failed",
            "response": {
                "status": "failed",
                "error": {
                    "code": "context_length_exceeded",
                    "message": "This model's maximum context length is 128000 tokens."
                }
            }
        }),
    )]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;
    let events = collect(
        &ResponsesStreamFn::new(server.uri(), "k"),
        &test_context(),
        StreamOptions::default(),
    )
    .await;
    assert_eq!(
        find_error_kind(&events),
        Some(Some(StreamErrorKind::ContextWindowExceeded)),
        "{events:?}"
    );
}

#[tokio::test]
async fn response_failed_content_filter_is_content_filtered() {
    let body = sse(&[(
        "response.failed",
        serde_json::json!({
            "type": "response.failed",
            "response": {
                "status": "failed",
                "error": {
                    "code": "content_filter",
                    "message": "The response was filtered due to policy."
                }
            }
        }),
    )]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;
    let events = collect(
        &ResponsesStreamFn::new(server.uri(), "k"),
        &test_context(),
        StreamOptions::default(),
    )
    .await;
    assert_eq!(
        find_error_kind(&events),
        Some(Some(StreamErrorKind::ContentFiltered)),
        "{events:?}"
    );
}

#[tokio::test]
async fn truncated_stream_without_completed_is_a_network_error() {
    let body = sse(&[(
        "response.output_text.delta",
        serde_json::json!({"type": "response.output_text.delta", "delta": "cut off"}),
    )]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;
    let events = collect(
        &ResponsesStreamFn::new(server.uri(), "k"),
        &test_context(),
        StreamOptions::default(),
    )
    .await;
    assert_eq!(
        names(&events),
        ["Start", "TextStart", "TextDelta", "TextEnd", "Error"],
        "{events:?}"
    );
    assert_eq!(
        find_error_kind(&events),
        Some(Some(StreamErrorKind::Network))
    );
    assert!(
        find_error_message(&events)
            .unwrap()
            .contains("ended unexpectedly")
    );
}

#[tokio::test]
async fn incomplete_max_output_tokens_is_done_with_length() {
    let body = sse(&[
        (
            "response.output_text.delta",
            serde_json::json!({"type": "response.output_text.delta", "delta": "long"}),
        ),
        (
            "response.incomplete",
            serde_json::json!({"type": "response.incomplete", "response": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}, "usage": {"input_tokens": 5, "output_tokens": 99}}}),
        ),
    ]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(&body))
        .mount(&server)
        .await;
    let events = collect(
        &ResponsesStreamFn::new(server.uri(), "k"),
        &test_context(),
        StreamOptions::default(),
    )
    .await;
    assert!(
        matches!(events.last(), Some(AssistantMessageEvent::Done { stop_reason: StopReason::Length, usage, .. }) if usage.output == 99),
        "{events:?}"
    );
}

#[tokio::test]
async fn http_429_maps_to_throttled_and_rate_limit_headers_reach_the_caller() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("x-codex-primary-used-percent", "98")
                .set_body_string(r#"{"error":{"message":"quota","type":"rate_limit_exceeded"}}"#),
        )
        .mount(&server)
        .await;
    let seen = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen);
    let options = StreamOptions::default()
        .with_on_rate_limit(Arc::new(move |s| *sink.lock().unwrap() = Some(s.clone())));
    let events = collect(
        &ResponsesStreamFn::new(server.uri(), "k"),
        &test_context(),
        options,
    )
    .await;
    assert_eq!(
        find_error_kind(&events),
        Some(Some(StreamErrorKind::Throttled)),
        "{events:?}"
    );
    assert_eq!(
        seen.lock().unwrap().as_ref().unwrap().used_percent,
        Some(98.0)
    );
}

#[tokio::test]
async fn static_headers_and_custom_path_reach_the_request() {
    let body = sse(&[completed(
        serde_json::json!({"input_tokens": 1, "output_tokens": 0}),
    )]);
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/backend-api/codex/responses"))
        .and(header("chatgpt-account-id", "acct-1"))
        .and(header("openai-beta", "responses=experimental"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(sse_response(&body))
        .expect(1)
        .mount(&server)
        .await;
    let stream_fn = ResponsesStreamFn::new(server.uri(), "test-key")
        .with_responses_path("/backend-api/codex/responses")
        .with_header(
            HeaderName::from_static("chatgpt-account-id"),
            HeaderValue::from_static("acct-1"),
        )
        .with_header(
            HeaderName::from_static("openai-beta"),
            HeaderValue::from_static("responses=experimental"),
        );
    let events = collect(&stream_fn, &test_context(), StreamOptions::default()).await;
    assert!(
        matches!(events.last(), Some(AssistantMessageEvent::Done { .. })),
        "{events:?}"
    );
}

#[tokio::test]
async fn pre_send_cancellation_skips_the_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(sse_response(""))
        .expect(0)
        .mount(&server)
        .await;
    let token = CancellationToken::new();
    token.cancel();
    let stream_fn = ResponsesStreamFn::new(server.uri(), "k");
    let model = test_model();
    let events = stream_fn
        .stream(&model, &test_context(), &StreamOptions::default(), token)
        .collect::<Vec<_>>()
        .await;
    assert_eq!(names(&events), ["Start", "Error"]);
    assert!(matches!(
        &events[1],
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Aborted,
            ..
        }
    ));
}
