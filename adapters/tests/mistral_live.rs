#![cfg(feature = "mistral")]
//! Live API tests for `MistralStreamFn`.
//!
//! These tests hit the real Mistral API and are skipped by default.
//! Run with: `cargo test -p swink-agent-adapters --test mistral_live -- --ignored`
//! Requires `MISTRAL_API_KEY` in `.env` or environment.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AgentMessage, AgentTool, AgentToolResult, AssistantMessageEvent, ContentBlock,
    LlmMessage, ModelSpec, StopReason, StreamFn, StreamOptions, UserMessage,
};
use swink_agent_adapters::MistralStreamFn;

// ── Constants ──────────────────────────────────────────────────────────────

const TIMEOUT: Duration = Duration::from_secs(30);

// ── Helpers ────────────────────────────────────────────────────────────────

fn mistral_key() -> String {
    dotenvy::dotenv().ok();
    std::env::var("MISTRAL_API_KEY").expect("MISTRAL_API_KEY must be set to run live tests")
}

fn cheap_model() -> ModelSpec {
    dotenvy::dotenv().ok();
    let model_id =
        std::env::var("MISTRAL_MODEL").unwrap_or_else(|_| "mistral-small-latest".to_string());
    ModelSpec::new("mistral", &model_id)
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
    sf: &MistralStreamFn,
    context: &AgentContext,
) -> Vec<AssistantMessageEvent> {
    let model = cheap_model();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    sf.stream(&model, context, &options, token)
        .collect::<Vec<_>>()
        .await
}

const fn event_name(event: &AssistantMessageEvent) -> &'static str {
    match event {
        AssistantMessageEvent::Start => "Start",
        AssistantMessageEvent::TextStart { .. } => "TextStart",
        AssistantMessageEvent::TextDelta { .. } => "TextDelta",
        AssistantMessageEvent::TextEnd { .. } => "TextEnd",
        AssistantMessageEvent::ToolCallStart { .. } => "ToolCallStart",
        AssistantMessageEvent::ToolCallDelta { .. } => "ToolCallDelta",
        AssistantMessageEvent::ToolCallEnd { .. } => "ToolCallEnd",
        AssistantMessageEvent::Done { .. } => "Done",
        AssistantMessageEvent::Error { .. } => "Error",
        _ => "Unknown",
    }
}

struct WeatherTool;

impl AgentTool for WeatherTool {
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
                is_error: false,
                transfer_signal: None,
            }
        })
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "hits live API"]
async fn live_text_stream() {
    let key = mistral_key();
    let sf = MistralStreamFn::new("https://api.mistral.ai", &key);
    let context = simple_context("What is 2+2?");

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out");

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(types.contains(&"Start"), "missing Start: {types:?}");
    assert!(types.contains(&"TextDelta"), "missing TextDelta: {types:?}");
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
async fn live_tool_use_stream() {
    let key = mistral_key();
    let sf = MistralStreamFn::new("https://api.mistral.ai", &key);
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
        tools: vec![Arc::new(WeatherTool)],
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

    // Verify the tool call ID is in harness format (remapped from Mistral's 9-char).
    let tool_id = events.iter().find_map(|e| match e {
        AssistantMessageEvent::ToolCallStart { id, .. } => Some(id.clone()),
        _ => None,
    });
    let id = tool_id.expect("missing tool call ID");
    assert!(
        id.starts_with("call_"),
        "expected harness-format ID, got: {id}"
    );

    let stop_reason = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(stop_reason, Some(StopReason::ToolUse));
}
