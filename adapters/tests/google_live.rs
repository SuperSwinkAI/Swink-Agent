#![cfg(feature = "gemini")]
//! Live API tests for `GeminiStreamFn`.
//!
//! These tests hit the real Google Gemini API and are skipped by default.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AgentMessage, AgentTool, AgentToolResult, ApiVersion, AssistantMessageEvent,
    ContentBlock, LlmMessage, ModelSpec, StopReason, StreamFn, StreamOptions, UserMessage,
};
use swink_agent_adapters::GeminiStreamFn;

const TIMEOUT: Duration = Duration::from_secs(30);

fn google_key() -> String {
    dotenvy::dotenv().ok();
    std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set to run live tests")
}

fn cheap_model() -> ModelSpec {
    dotenvy::dotenv().ok();
    let model_id =
        std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-3-flash-preview".to_string());
    ModelSpec::new("google", &model_id)
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

async fn collect_events(
    stream_fn: &GeminiStreamFn,
    context: &AgentContext,
) -> Vec<AssistantMessageEvent> {
    let model = cheap_model();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    stream_fn
        .stream(&model, context, &options, token)
        .collect::<Vec<_>>()
        .await
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
                    "city": { "type": "string" }
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
                is_error: false,
            }
        })
    }
}

#[tokio::test]
#[ignore = "hits live API"]
async fn live_text_stream() {
    let key = google_key();
    let stream_fn = GeminiStreamFn::new(
        "https://generativelanguage.googleapis.com",
        &key,
        ApiVersion::V1beta,
    );
    let context = simple_context("What is 2+2?");

    let events = timeout(TIMEOUT, collect_events(&stream_fn, &context))
        .await
        .expect("timed out");

    let types: Vec<_> = events.iter().map(event_name).collect();
    assert!(types.contains(&"Start"), "missing Start: {types:?}");
    assert!(types.contains(&"TextStart"), "missing TextStart: {types:?}");
    assert!(types.contains(&"TextDelta"), "missing TextDelta: {types:?}");
    assert!(types.contains(&"TextEnd"), "missing TextEnd: {types:?}");
    assert!(types.contains(&"Done"), "missing Done: {types:?}");
}

#[tokio::test]
#[ignore = "hits live API"]
async fn live_tool_use_stream() {
    let key = google_key();
    let stream_fn = GeminiStreamFn::new(
        "https://generativelanguage.googleapis.com",
        &key,
        ApiVersion::V1beta,
    );
    let context = AgentContext {
        system_prompt: "You must use the get_weather tool to answer.".into(),
        messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "What's the weather in Paris?".into(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))],
        tools: vec![Arc::new(DummyTool)],
    };

    let events = timeout(TIMEOUT, collect_events(&stream_fn, &context))
        .await
        .expect("timed out");

    let types: Vec<_> = events.iter().map(event_name).collect();
    assert!(
        types.contains(&"ToolCallStart"),
        "missing ToolCallStart: {types:?}"
    );
    assert!(
        types.contains(&"ToolCallEnd"),
        "missing ToolCallEnd: {types:?}"
    );

    let stop_reason = events.iter().find_map(|event| match event {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(stop_reason, Some(StopReason::ToolUse));
}
