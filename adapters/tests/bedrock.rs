#![cfg(feature = "bedrock")]
//! Wiremock-based tests for `BedrockStreamFn`.
//!
//! These tests simulate the Bedrock ConverseStream API by returning
//! binary event-stream frames encoded with `aws-smithy-eventstream`.

use aws_smithy_eventstream::frame::write_message_to;
use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
use bytes::BytesMut;
use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use swink_agent::{
    AgentContext, AgentMessage, AgentTool, AgentToolResult, AssistantMessageEvent, ContentBlock,
    LlmMessage, ModelSpec, StopReason, StreamFn, StreamOptions, UserMessage,
};
use swink_agent_adapters::BedrockStreamFn;

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
            &AgentContext {
                system_prompt: String::new(),
                messages: Vec::new(),
                tools: Vec::new(),
            },
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
        Box::pin(async {
            AgentToolResult {
                content: vec![],
                details: serde_json::Value::Null,
                is_error: false,
                transfer_signal: None,
            }
        })
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
            &AgentContext {
                system_prompt: String::new(),
                messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                    content: vec![ContentBlock::Text {
                        text: "weather?".into(),
                    }],
                    timestamp: 0,
                    cache_hint: None,
                }))],
                tools: vec![std::sync::Arc::new(DummyTool)],
            },
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
