#![cfg(feature = "anthropic")]
//! Live API tests for `AnthropicStreamFn`.
//!
//! These tests hit the real Anthropic API and are skipped by default.
//! Run with: `cargo test -p swink-agent-adapters --test anthropic_live -- --ignored`
//! Requires `ANTHROPIC_API_KEY` in `.env` or environment.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AgentMessage, AgentTool, AgentToolResult, AssistantMessage,
    AssistantMessageEvent, ContentBlock, Cost, LlmMessage, ModelSpec, StopReason, StreamFn,
    StreamOptions, ThinkingLevel, Usage, UserMessage,
};
use swink_agent_adapters::AnthropicStreamFn;

// ── Constants ────────────────────────────────────────────────────────────────

const TIMEOUT: Duration = Duration::from_secs(30);

// ── Helpers ──────────────────────────────────────────────────────────────────

fn anthropic_key() -> String {
    dotenvy::dotenv().ok();
    std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set to run live tests")
}

fn cheap_model() -> ModelSpec {
    dotenvy::dotenv().ok();
    let model_id = std::env::var("ANTHROPIC_MODEL")
        .unwrap_or_else(|_| "claude-haiku-4-5-20251001".to_string());
    ModelSpec::new("anthropic", &model_id)
}

fn simple_context(prompt: &str) -> AgentContext {
    AgentContext {
        system_prompt: "Reply in one word.".into(),
        messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
            }],
            timestamp: 0,
        }))],
        tools: Vec::new(),
    }
}

async fn collect_events(
    sf: &AnthropicStreamFn,
    context: &AgentContext,
) -> Vec<AssistantMessageEvent> {
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
        // Leak a static reference so we can return &Value from the trait.
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
            }
        })
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "hits live API"]
async fn live_text_stream() {
    let key = anthropic_key();
    let sf = AnthropicStreamFn::new("https://api.anthropic.com", &key);
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

    // Should have some actual text content
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
async fn live_usage_and_cost() {
    let key = anthropic_key();
    let sf = AnthropicStreamFn::new("https://api.anthropic.com", &key);
    let context = simple_context("Say hello.");

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out");

    let (stop_reason, usage) = events
        .iter()
        .find_map(|e| match e {
            AssistantMessageEvent::Done {
                stop_reason, usage, ..
            } => Some((*stop_reason, usage.clone())),
            _ => None,
        })
        .expect("missing Done event");

    assert_eq!(stop_reason, StopReason::Stop);
    assert!(usage.input > 0, "expected non-zero input tokens");
    assert!(usage.output > 0, "expected non-zero output tokens");
}

#[tokio::test]
#[ignore = "hits live API"]
async fn live_tool_use_stream() {
    let key = anthropic_key();
    let sf = AnthropicStreamFn::new("https://api.anthropic.com", &key);
    let context = AgentContext {
        system_prompt: "You must use the get_weather tool to answer. Do not reply with text only."
            .into(),
        messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "What's the weather in Paris?".into(),
            }],
            timestamp: 0,
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

    // Verify the tool name
    let tool_name = events.iter().find_map(|e| match e {
        AssistantMessageEvent::ToolCallStart { name, .. } => Some(name.clone()),
        _ => None,
    });
    assert_eq!(tool_name.as_deref(), Some("get_weather"));

    // Stop reason should be ToolUse
    let stop_reason = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(stop_reason, Some(StopReason::ToolUse));
}

#[tokio::test]
#[ignore = "hits live API"]
async fn live_thinking_stream() {
    let key = anthropic_key();
    let sf = AnthropicStreamFn::new("https://api.anthropic.com", &key);
    let mut model = cheap_model();
    model.thinking_level = ThinkingLevel::Low;

    let context = simple_context("What is 7 * 8?");
    let options = StreamOptions::default();
    let token = CancellationToken::new();

    let events = timeout(TIMEOUT, async {
        sf.stream(&model, &context, &options, token)
            .collect::<Vec<_>>()
            .await
    })
    .await
    .expect("timed out");

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(
        types.contains(&"ThinkingStart"),
        "missing ThinkingStart: {types:?}"
    );
    assert!(
        types.contains(&"ThinkingDelta"),
        "missing ThinkingDelta: {types:?}"
    );
    assert!(
        types.contains(&"ThinkingEnd"),
        "missing ThinkingEnd: {types:?}"
    );
    assert!(types.contains(&"TextStart"), "missing TextStart: {types:?}");
    assert!(types.contains(&"Done"), "missing Done: {types:?}");

    // Thinking should come before text
    let thinking_end_pos = types.iter().position(|&t| t == "ThinkingEnd").unwrap();
    let text_start_pos = types.iter().position(|&t| t == "TextStart").unwrap();
    assert!(
        thinking_end_pos < text_start_pos,
        "ThinkingEnd should precede TextStart"
    );
}

#[tokio::test]
#[ignore = "hits live API"]
async fn live_multi_turn_context() {
    let key = anthropic_key();
    let sf = AnthropicStreamFn::new("https://api.anthropic.com", &key);

    // First turn
    let context = simple_context("My name is Alice.");
    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out on first turn");

    // Extract the assistant's reply text
    let reply: String = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert!(!reply.is_empty(), "first turn should produce text");

    // Second turn: include assistant reply + follow-up
    let context = AgentContext {
        system_prompt: "Reply in one short sentence.".into(),
        messages: vec![
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "My name is Alice.".into(),
                }],
                timestamp: 0,
            })),
            AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
                content: vec![ContentBlock::Text { text: reply }],
                provider: "anthropic".into(),
                model_id: cheap_model().model_id,
                usage: Usage::default(),
                cost: Cost::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: 1,
            })),
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "What is my name?".into(),
                }],
                timestamp: 2,
            })),
        ],
        tools: Vec::new(),
    };

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out on second turn");

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(
        types.contains(&"Done"),
        "missing Done on second turn: {types:?}"
    );

    let reply: String = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    let reply_lower = reply.to_lowercase();
    assert!(
        reply_lower.contains("alice"),
        "expected 'Alice' in multi-turn reply, got: {reply}"
    );
}

#[tokio::test]
#[ignore = "hits live API"]
async fn live_stop_reason_mapping() {
    let key = anthropic_key();
    let sf = AnthropicStreamFn::new("https://api.anthropic.com", &key);
    let context = simple_context("Say yes.");

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out");

    let stop_reason = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(
        stop_reason,
        Some(StopReason::Stop),
        "expected Stop for a simple prompt"
    );
}

#[tokio::test]
#[ignore = "hits live API"]
async fn live_invalid_key_returns_auth_error() {
    let sf = AnthropicStreamFn::new("https://api.anthropic.com", "sk-ant-bogus-key-12345");
    let context = simple_context("Hi.");

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out");

    let err = events
        .iter()
        .find_map(|e| match e {
            AssistantMessageEvent::Error { error_message, .. } => Some(error_message.clone()),
            _ => None,
        })
        .expect("expected Error event for invalid key");
    assert!(
        err.to_lowercase().contains("auth"),
        "expected auth-related error, got: {err}"
    );
}
