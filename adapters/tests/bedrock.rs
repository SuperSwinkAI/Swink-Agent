#![cfg(feature = "bedrock")]
//! Wiremock-based tests for `BedrockStreamFn`.

use futures::StreamExt;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use swink_agent::{
    AgentContext, AgentMessage, AgentTool, AgentToolResult, AssistantMessageEvent, ContentBlock,
    LlmMessage, ModelSpec, StopReason, StreamFn, StreamOptions, UserMessage,
};
use swink_agent_adapters::BedrockStreamFn;

#[tokio::test]
async fn bedrock_text_response_maps_to_text_events() {
    let response = r#"{"output":{"message":{"content":[{"text":"hello"}]}},"stopReason":"end_turn","usage":{"inputTokens":4,"outputTokens":2,"totalTokens":6}}"#;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/model/amazon.nova-pro-v1:0/converse"))
        .and(header_exists("authorization"))
        .and(header_exists("x-amz-date"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/json")
                .set_body_string(response),
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
        )
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
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async {
            AgentToolResult {
                content: vec![],
                details: serde_json::Value::Null,
                is_error: false,
            }
        })
    }
}

#[tokio::test]
async fn bedrock_tool_use_maps_to_tool_events() {
    let response = r#"{"output":{"message":{"content":[{"toolUse":{"toolUseId":"tool_1","name":"get_weather","input":{"city":"Paris"}}}]}},"stopReason":"tool_use","usage":{"inputTokens":4,"outputTokens":2,"totalTokens":6}}"#;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/model/us.anthropic.claude-sonnet-4-5-20250929-v1:0/converse",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/json")
                .set_body_string(response),
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
                }))],
                tools: vec![std::sync::Arc::new(DummyTool)],
            },
            &StreamOptions::default(),
            CancellationToken::new(),
        )
        .collect::<Vec<_>>()
        .await;

    assert!(events.iter().any(
        |e| matches!(e, AssistantMessageEvent::ToolCallStart { name, .. } if name == "get_weather")
    ));
    assert!(events.iter().any(|e| matches!(
        e,
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            ..
        }
    )));
}
