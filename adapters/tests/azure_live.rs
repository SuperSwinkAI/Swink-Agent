#![cfg(feature = "azure")]
//! Live API tests for `AzureStreamFn`.
//!
//! These tests hit a real Azure OpenAI deployment and are skipped by default.
//! Run with: `cargo test -p swink-agent-adapters --test azure_live -- --ignored`
//! Requires `AZURE_BASE_URL` and `AZURE_API_KEY` in `.env` or environment.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AgentMessage, AgentTool, AgentToolResult, AssistantMessageEvent, ContentBlock,
    LlmMessage, ModelSpec, StreamFn, StreamOptions, UserMessage,
};
use swink_agent_adapters::{AzureAuth, AzureStreamFn};

// ── Constants ────────────────────────────────────────────────────────────────

const TIMEOUT: Duration = Duration::from_secs(30);

// ── Helpers ──────────────────────────────────────────────────────────────────

fn stream_fn() -> AzureStreamFn {
    dotenvy::dotenv().ok();
    AzureStreamFn::new(
        std::env::var("AZURE_BASE_URL").expect("AZURE_BASE_URL must be set"),
        AzureAuth::ApiKey(std::env::var("AZURE_API_KEY").expect("AZURE_API_KEY must be set")),
    )
}

fn cheap_model() -> ModelSpec {
    dotenvy::dotenv().ok();
    let model_id = std::env::var("AZURE_MODEL").unwrap_or_else(|_| "gpt-5.4".to_string());
    ModelSpec::new("azure", &model_id)
}

fn simple_context(prompt: &str) -> AgentContext {
    AgentContext {
        system_prompt: "Reply in one word.".into(),
        messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))],
        tools: Vec::new(),
    }
}

async fn collect_events(sf: &AzureStreamFn, context: &AgentContext) -> Vec<AssistantMessageEvent> {
    let model = cheap_model();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    let stream = sf.stream(&model, context, &options, token);
    stream.collect::<Vec<_>>().await
}

const fn event_name(event: &AssistantMessageEvent) -> &'static str {
    match event {
        AssistantMessageEvent::Start => "Start",
        AssistantMessageEvent::TextStart { .. } => "TextStart",
        AssistantMessageEvent::TextDelta { .. } => "TextDelta",
        AssistantMessageEvent::TextEnd { .. } => "TextEnd",
        AssistantMessageEvent::ThinkingStart { .. } => "ThinkingStart",
        AssistantMessageEvent::ThinkingDelta { .. } => "ThinkingDelta",
        AssistantMessageEvent::ThinkingEnd { .. } => "ThinkingEnd",
        AssistantMessageEvent::ToolCallStart { .. } => "ToolCallStart",
        AssistantMessageEvent::ToolCallDelta { .. } => "ToolCallDelta",
        AssistantMessageEvent::ToolCallEnd { .. } => "ToolCallEnd",
        AssistantMessageEvent::Done { .. } => "Done",
        AssistantMessageEvent::Error { .. } => "Error",
        _ => "Unknown",
    }
}

/// A dummy tool for triggering tool-use responses.
struct DummyTool;

impl AgentTool for DummyTool {
    fn name(&self) -> &'static str {
        "get_weather"
    }

    fn label(&self) -> &'static str {
        "Get Weather"
    }

    fn description(&self) -> &'static str {
        "Get the current weather for a city."
    }

    fn parameters_schema(&self) -> &serde_json::Value {
        static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| {
            json!({
                "type": "object",
                "properties": {
                    "city": {
                        "type": "string",
                        "description": "The city name"
                    }
                },
                "required": ["city"]
            })
        })
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
                content: vec![ContentBlock::Text {
                    text: "72°F, sunny".into(),
                }],
                details: json!({}),
                is_error: false, transfer_signal: None,
            }
        })
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "hits live API"]
async fn live_text_stream() {
    let sf = stream_fn();
    let context = simple_context("What is 2+2?");

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out");

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(types.contains(&"Start"), "missing Start: {types:?}");
    assert!(types.contains(&"TextStart"), "missing TextStart: {types:?}");
    assert!(types.contains(&"TextDelta"), "missing TextDelta: {types:?}");
    assert!(types.contains(&"TextEnd"), "missing TextEnd: {types:?}");
    assert!(types.contains(&"Done"), "missing Done: {types:?}");

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert!(!text.is_empty(), "expected non-empty text response");
}

#[tokio::test]
#[ignore = "hits live API"]
async fn live_tool_call_stream() {
    let sf = stream_fn();
    let context = AgentContext {
        system_prompt: "You must use the get_weather tool to answer. Do not reply with text only."
            .into(),
        messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "What's the weather in Paris?".into(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))],
        tools: vec![Arc::new(DummyTool)],
    };

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out");

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(
        types.contains(&"ToolCallStart"),
        "missing ToolCallStart: {types:?}"
    );
    assert!(
        types.contains(&"ToolCallEnd"),
        "missing ToolCallEnd: {types:?}"
    );

    let tool_name = events.iter().find_map(|e| match e {
        AssistantMessageEvent::ToolCallStart { name, .. } => Some(name.clone()),
        _ => None,
    });
    assert_eq!(tool_name.as_deref(), Some("get_weather"));
}

#[tokio::test]
#[ignore = "hits live API"]
async fn live_invalid_key_returns_error() {
    let sf = AzureStreamFn::new(
        {
            dotenvy::dotenv().ok();
            std::env::var("AZURE_BASE_URL").expect("AZURE_BASE_URL must be set")
        },
        AzureAuth::ApiKey("bogus-api-key-12345".into()),
    );
    let context = simple_context("Hi.");

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out");

    let has_error = events
        .iter()
        .any(|e| matches!(e, AssistantMessageEvent::Error { .. }));
    assert!(has_error, "expected Error event for invalid key");
}
