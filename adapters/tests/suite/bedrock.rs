//! Wiremock-based tests for `BedrockStreamFn`.
//!
//! These tests simulate the Bedrock ConverseStream API by returning
//! binary event-stream frames encoded with `aws-smithy-eventstream`.

use aws_smithy_eventstream::frame::write_message_to;
use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
use bytes::BytesMut;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use swink_agent::{
    AgentContext, AgentMessage, AgentTool, AgentToolResult, AssistantMessageEvent, ContentBlock,
    LlmMessage, ModelSpec, StopReason, StreamFn, StreamOptions, UserMessage,
};
use swink_agent_adapters::BedrockStreamFn;

use crate::common::notify_on_request;

/// Encode a smithy event-stream `Message` into raw bytes.
fn encode_message(msg: &Message) -> Vec<u8> {
    let mut buf = BytesMut::new();
    write_message_to(msg, &mut buf).expect("failed to encode event-stream message");
    buf.to_vec()
}

/// Build an event-stream event message with the given event-type and JSON payload.
fn event_frame(event_type: &str, payload: &[u8]) -> Vec<u8> {
    let msg = Message::new_from_parts(
        vec![
            Header::new(
                ":message-type",
                HeaderValue::String(String::from("event").into()),
            ),
            Header::new(
                ":event-type",
                HeaderValue::String(String::from(event_type).into()),
            ),
            Header::new(
                ":content-type",
                HeaderValue::String(String::from("application/json").into()),
            ),
        ],
        bytes::Bytes::from(payload.to_vec()),
    );
    encode_message(&msg)
}

/// Build a complete text-response event stream body.
fn text_response_body(text: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(event_frame("messageStart", br#"{"role":"assistant"}"#));
    body.extend(event_frame(
        "contentBlockStart",
        br#"{"contentBlockIndex":0,"start":{"type":"text"}}"#,
    ));
    body.extend(event_frame(
        "contentBlockDelta",
        format!(r#"{{"contentBlockIndex":0,"delta":{{"type":"text","text":"{text}"}}}}"#)
            .as_bytes(),
    ));
    body.extend(event_frame(
        "contentBlockStop",
        br#"{"contentBlockIndex":0}"#,
    ));
    body.extend(event_frame("messageStop", br#"{"stopReason":"end_turn"}"#));
    body.extend(event_frame(
        "metadata",
        br#"{"usage":{"inputTokens":4,"outputTokens":2,"totalTokens":6}}"#,
    ));
    body
}

/// Build a complete tool-use event stream body.
fn tool_use_response_body(tool_id: &str, tool_name: &str, args_json: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(event_frame("messageStart", br#"{"role":"assistant"}"#));
    body.extend(event_frame(
        "contentBlockStart",
        format!(
            r#"{{"contentBlockIndex":0,"start":{{"type":"toolUse","toolUseId":"{tool_id}","name":"{tool_name}"}}}}"#
        )
        .as_bytes(),
    ));
    body.extend(event_frame(
        "contentBlockDelta",
        format!(r#"{{"contentBlockIndex":0,"delta":{{"type":"toolUse","input":{args_json}}}}}"#)
            .as_bytes(),
    ));
    body.extend(event_frame(
        "contentBlockStop",
        br#"{"contentBlockIndex":0}"#,
    ));
    body.extend(event_frame("messageStop", br#"{"stopReason":"tool_use"}"#));
    body.extend(event_frame(
        "metadata",
        br#"{"usage":{"inputTokens":4,"outputTokens":2,"totalTokens":6}}"#,
    ));
    body
}

#[tokio::test]
async fn bedrock_text_response_maps_to_text_events() {
    let body = text_response_body("hello");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/model/amazon.nova-pro-v1:0/converse-stream"))
        .and(header_exists("authorization"))
        .and(header_exists("x-amz-date"))
        .and(header_exists("x-amz-content-sha256"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/vnd.amazon.eventstream")
                .set_body_bytes(body),
        )
        .mount(&server)
        .await;

    let stream_fn = BedrockStreamFn::new_with_base_url(
        server.uri(),
        "us-east-1",
        "AKIDEXAMPLE",
        "secret",
        None,
    );
    let events = stream_fn
        .stream(
            &ModelSpec::new("bedrock", "amazon.nova-pro-v1:0"),
            &AgentContext::new(String::new(), Vec::new(), Vec::new()),
            &StreamOptions::default(),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;

    assert!(
        events.iter().any(
            |e| matches!(e, AssistantMessageEvent::TextDelta { delta, .. } if delta == "hello")
        ),
        "expected TextDelta with 'hello', got: {events:?}"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            ..
        }
    )));
}

#[tokio::test]
async fn bedrock_http_400_input_too_long_sets_context_overflow_kind() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/model/amazon.nova-pro-v1:0/converse-stream"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string(r#"{"message":"Input is too long for requested model."}"#),
        )
        .mount(&server)
        .await;

    let stream_fn = BedrockStreamFn::new_with_base_url(
        server.uri(),
        "us-east-1",
        "AKIDEXAMPLE",
        "secret",
        None,
    );
    let events = stream_fn
        .stream(
            &ModelSpec::new("bedrock", "amazon.nova-pro-v1:0"),
            &AgentContext::new(String::new(), Vec::new(), Vec::new()),
            &StreamOptions::default(),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;

    assert!(
        matches!(events.first(), Some(AssistantMessageEvent::Start)),
        "pre-stream HTTP failures must start with Start: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AssistantMessageEvent::Error {
                error_kind: Some(swink_agent::StreamErrorKind::ContextWindowExceeded),
                ..
            }
        )),
        "expected structured ContextWindowExceeded, got: {events:?}"
    );
}

#[tokio::test]
async fn bedrock_http_400_other_validation_error_has_no_kind() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/model/amazon.nova-pro-v1:0/converse-stream"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string(r#"{"message":"The provided model identifier is invalid."}"#),
        )
        .mount(&server)
        .await;

    let stream_fn = BedrockStreamFn::new_with_base_url(
        server.uri(),
        "us-east-1",
        "AKIDEXAMPLE",
        "secret",
        None,
    );
    let events = stream_fn
        .stream(
            &ModelSpec::new("bedrock", "amazon.nova-pro-v1:0"),
            &AgentContext::new(String::new(), Vec::new(), Vec::new()),
            &StreamOptions::default(),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;

    assert!(
        events.iter().any(|event| matches!(
            event,
            AssistantMessageEvent::Error {
                error_kind: None,
                ..
            }
        )),
        "non-overflow 400 must stay unclassified, got: {events:?}"
    );
}

#[tokio::test]
async fn bedrock_pre_request_cancellation_is_aborted() {
    let token = CancellationToken::new();
    token.cancel();

    let stream_fn = BedrockStreamFn::new_with_base_url(
        "http://127.0.0.1:9",
        "us-east-1",
        "AKIDEXAMPLE",
        "secret",
        None,
    );
    let events = stream_fn
        .stream(
            &ModelSpec::new("bedrock", "amazon.nova-pro-v1:0"),
            &AgentContext::new(String::new(), Vec::new(), Vec::new()),
            &StreamOptions::default(),
            token,
        )
        .collect::<Vec<_>>()
        .await;

    assert!(events.iter().any(|event| matches!(
        event,
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Aborted,
            error_kind: None,
            ..
        }
    )));
}

struct DummyTool;

impl AgentTool for DummyTool {
    fn name(&self) -> &'static str {
        "get_weather"
    }
    fn label(&self) -> &'static str {
        "Get Weather"
    }
    fn description(&self) -> &'static str {
        "Get weather."
    }
    fn parameters_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| serde_json::json!({"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}))
    }
    fn execute(
        &self,
        _tool_call_id: &str,
        _params: serde_json::Value,
        _cancellation_token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        _state: std::sync::Arc<std::sync::RwLock<swink_agent::SessionState>>,
        _credential: Option<swink_agent::ResolvedCredential>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async { AgentToolResult::new(vec![], false) })
    }
}

#[tokio::test]
async fn bedrock_tool_use_maps_to_tool_events() {
    let body = tool_use_response_body("tool_1", "get_weather", r#""{\"city\":\"Paris\"}""#);

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/model/us.anthropic.claude-sonnet-4-5-20250929-v1:0/converse-stream",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/vnd.amazon.eventstream")
                .set_body_bytes(body),
        )
        .mount(&server)
        .await;

    let stream_fn = BedrockStreamFn::new_with_base_url(
        server.uri(),
        "us-east-1",
        "AKIDEXAMPLE",
        "secret",
        None,
    );
    let events = stream_fn
        .stream(
            &ModelSpec::new("bedrock", "us.anthropic.claude-sonnet-4-5-20250929-v1:0"),
            &AgentContext::new(
                String::new(),
                vec![AgentMessage::Llm(LlmMessage::User(
                    UserMessage::new(vec![ContentBlock::Text {
                        text: "weather?".into(),
                    }])
                    .with_timestamp(0),
                ))],
                vec![std::sync::Arc::new(DummyTool)],
            ),
            &StreamOptions::default(),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, AssistantMessageEvent::ToolCallStart { name, .. } if name == "get_weather")),
        "expected ToolCallStart with 'get_weather', got: {events:?}"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            ..
        }
    )));
}

#[tokio::test]
async fn bedrock_session_token_is_forwarded() {
    let body = text_response_body("hello");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/model/amazon.nova-pro-v1:0/converse-stream"))
        .and(header("x-amz-security-token", "session-token"))
        .and(header_exists("authorization"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/vnd.amazon.eventstream")
                .set_body_bytes(body),
        )
        .mount(&server)
        .await;

    let stream_fn = BedrockStreamFn::new_with_base_url(
        server.uri(),
        "us-east-1",
        "AKIDEXAMPLE",
        "secret",
        Some("session-token".to_string()),
    );
    let events = stream_fn
        .stream(
            &ModelSpec::new("bedrock", "amazon.nova-pro-v1:0"),
            &AgentContext::new(String::new(), Vec::new(), Vec::new()),
            &StreamOptions::default(),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;

    assert!(events.iter().any(|e| matches!(
        e,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::Stop,
            ..
        }
    )));
}

#[tokio::test]
async fn bedrock_stream_cancellation_is_aborted() {
    let server = MockServer::start().await;
    let (slow_response, request_seen) = notify_on_request(
        ResponseTemplate::new(200)
            .insert_header("Content-Type", "application/vnd.amazon.eventstream")
            .set_body_bytes(text_response_body("hello"))
            .set_delay(std::time::Duration::from_secs(30)),
    );

    Mock::given(method("POST"))
        .and(path("/model/amazon.nova-pro-v1:0/converse-stream"))
        .respond_with(slow_response)
        .mount(&server)
        .await;

    let stream_fn = BedrockStreamFn::new_with_base_url(
        server.uri(),
        "us-east-1",
        "AKIDEXAMPLE",
        "secret",
        None,
    );
    let token = CancellationToken::new();
    let cancel_token = token.clone();
    let events_handle = tokio::spawn(async move {
        let model = ModelSpec::new("bedrock", "amazon.nova-pro-v1:0");
        let context = AgentContext::new(String::new(), Vec::new(), Vec::new());
        let options = StreamOptions::default();
        stream_fn
            .stream(&model, &context, &options, token)
            .collect::<Vec<_>>()
            .await
    });

    request_seen.notified().await;
    cancel_token.cancel();
    let events = events_handle.await.expect("stream task should complete");

    assert!(events.iter().any(|event| matches!(
        event,
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Aborted,
            error_kind: None,
            ..
        }
    )));
}

#[tokio::test]
async fn bedrock_startup_cancellation_does_not_wait_for_send() {
    let server = MockServer::start().await;
    let (slow_response, request_seen) = notify_on_request(
        ResponseTemplate::new(200)
            .insert_header("Content-Type", "application/vnd.amazon.eventstream")
            .set_body_bytes(text_response_body("hello"))
            .set_delay(std::time::Duration::from_secs(30)),
    );

    Mock::given(method("POST"))
        .and(path("/model/amazon.nova-pro-v1:0/converse-stream"))
        .respond_with(slow_response)
        .mount(&server)
        .await;

    let stream_fn = BedrockStreamFn::new_with_base_url(
        server.uri(),
        "us-east-1",
        "AKIDEXAMPLE",
        "secret",
        None,
    );
    let token = CancellationToken::new();
    let cancel_token = token.clone();
    let events_handle = tokio::spawn(async move {
        let model = ModelSpec::new("bedrock", "amazon.nova-pro-v1:0");
        let context = AgentContext::new(String::new(), Vec::new(), Vec::new());
        let options = StreamOptions::default();
        stream_fn
            .stream(&model, &context, &options, token)
            .collect::<Vec<_>>()
            .await
    });

    request_seen.notified().await;
    cancel_token.cancel();
    let events = events_handle.await.expect("stream task should complete");

    assert!(matches!(events.first(), Some(AssistantMessageEvent::Start)));
    assert!(events.iter().any(|event| matches!(
        event,
        AssistantMessageEvent::Error {
            stop_reason: StopReason::Aborted,
            error_kind: None,
            error_message,
            ..
        } if error_message.contains("Bedrock request cancelled")
    )));
}

/// `ServingOptions::extra` maps onto the Converse API's
/// `additionalModelRequestFields` — its verbatim pass-through for
/// model-native parameters. Keys colliding with the typed base parameters
/// (`temperature`, `maxTokens`) are dropped: typed fields win.
#[tokio::test]
async fn bedrock_extra_maps_to_additional_model_request_fields() {
    let body = text_response_body("ok");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/model/amazon.nova-pro-v1:0/converse-stream"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/vnd.amazon.eventstream")
                .set_body_bytes(body),
        )
        .expect(1)
        .mount(&server)
        .await;

    let stream_fn = BedrockStreamFn::new_with_base_url(
        server.uri(),
        "us-east-1",
        "AKIDEXAMPLE",
        "secret",
        None,
    );
    let options = StreamOptions::default().with_temperature(0.7).with_serving(
        swink_agent::ServingOptions::default().with_extra(
            [
                ("top_k".to_string(), serde_json::json!(250)),
                // Colliding keys: the typed base parameters must win and the
                // extra entries must not reach `additionalModelRequestFields`.
                ("temperature".to_string(), serde_json::json!(0.1)),
                ("maxTokens".to_string(), serde_json::json!(1)),
            ]
            .into_iter()
            .collect(),
        ),
    );
    let _events = stream_fn
        .stream(
            &ModelSpec::new("bedrock", "amazon.nova-pro-v1:0"),
            &AgentContext::new(String::new(), Vec::new(), Vec::new()),
            &options,
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;

    let requests = server.received_requests().await.expect("request log");
    let request: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
    let additional = &request["additionalModelRequestFields"];
    assert_eq!(
        additional["top_k"],
        serde_json::json!(250),
        "body: {request}"
    );
    assert!(
        additional.get("temperature").is_none() && additional.get("maxTokens").is_none(),
        "typed base parameters must be filtered from additionalModelRequestFields: {request}"
    );
    assert_eq!(
        request["inferenceConfig"]["temperature"],
        serde_json::json!(0.7),
        "typed `temperature` must win via inferenceConfig: {request}"
    );
}

/// With `extra` empty the request body carries no
/// `additionalModelRequestFields` key at all.
#[tokio::test]
async fn bedrock_empty_extra_omits_additional_model_request_fields() {
    let body = text_response_body("ok");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/model/amazon.nova-pro-v1:0/converse-stream"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/vnd.amazon.eventstream")
                .set_body_bytes(body),
        )
        .expect(1)
        .mount(&server)
        .await;

    let stream_fn = BedrockStreamFn::new_with_base_url(
        server.uri(),
        "us-east-1",
        "AKIDEXAMPLE",
        "secret",
        None,
    );
    let _events = stream_fn
        .stream(
            &ModelSpec::new("bedrock", "amazon.nova-pro-v1:0"),
            &AgentContext::new(String::new(), Vec::new(), Vec::new()),
            &StreamOptions::default(),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;

    let requests = server.received_requests().await.expect("request log");
    let request: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
    assert!(
        request.get("additionalModelRequestFields").is_none(),
        "empty extra must not emit additionalModelRequestFields: {request}"
    );
}
