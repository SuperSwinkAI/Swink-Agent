#![cfg(feature = "ollama")]
//! Live integration tests for `OllamaStreamFn`.
//!
//! These tests hit a real Ollama instance and are skipped by default.
//! Run with: `cargo test -p swink-agent-adapters --test ollama_live -- --ignored`
//! Requires a running Ollama server at `http://localhost:11434` with `llama3.2:1b` pulled.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use serde_json::json;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AgentMessage, AgentTool, AgentToolResult, AssistantMessage,
    AssistantMessageEvent, ContentBlock, Cost, LlmMessage, ModelSpec, StopReason, StreamFn,
    StreamOptions, Usage, UserMessage,
};
use swink_agent_adapters::OllamaStreamFn;

// ── Constants ────────────────────────────────────────────────────────────────

const TIMEOUT: Duration = Duration::from_secs(60);

// ── Helpers ──────────────────────────────────────────────────────────────────

fn ollama() -> OllamaStreamFn {
    OllamaStreamFn::new("http://localhost:11434")
}

fn model() -> ModelSpec {
    ModelSpec::new("ollama", "llama3.2:1b")
}

fn simple_context(prompt: &str) -> AgentContext {
    AgentContext {
        system_prompt: "Reply in one short sentence.".into(),
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

async fn collect_events(sf: &OllamaStreamFn, context: &AgentContext) -> Vec<AssistantMessageEvent> {
    let m = model();
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    let stream = sf.stream(&m, context, &options, token);
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
struct DummyWeatherTool;

impl AgentTool for DummyWeatherTool {
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
            }
        })
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "hits live Ollama instance"]
async fn live_text_stream() {
    let sf = ollama();
    let context = simple_context("Say hello.");

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
#[ignore = "hits live Ollama instance"]
async fn live_usage_captured() {
    let sf = ollama();
    let context = simple_context("Say hello.");

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out");

    let (usage, cost) = events
        .iter()
        .find_map(|e| match e {
            AssistantMessageEvent::Done { usage, cost, .. } => Some((usage.clone(), cost.clone())),
            _ => None,
        })
        .expect("missing Done event");

    assert!(usage.input > 0, "expected non-zero input tokens");
    assert!(usage.output > 0, "expected non-zero output tokens");
    assert!(
        cost.total.abs() < f64::EPSILON,
        "Ollama is local — cost should be zero, got: {}",
        cost.total
    );
}

#[tokio::test]
#[ignore = "hits live Ollama instance"]
async fn live_tool_use_stream() {
    let sf = ollama();
    let context = AgentContext {
        system_prompt: "You must use the get_weather tool to answer. Do not reply with text only."
            .into(),
        messages: vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
            content: vec![ContentBlock::Text {
                text: "What's the weather in Paris? Use the get_weather tool.".into(),
            }],
            timestamp: 0,
            cache_hint: None,
        }))],
        tools: vec![Arc::new(DummyWeatherTool)],
    };

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out");

    let types: Vec<&str> = events.iter().map(|e| event_name(e)).collect();
    assert!(
        types.contains(&"ToolCallStart"),
        "missing ToolCallStart: {types:?}"
    );

    let tool_name = events.iter().find_map(|e| match e {
        AssistantMessageEvent::ToolCallStart { name, .. } => Some(name.clone()),
        _ => None,
    });
    assert_eq!(tool_name.as_deref(), Some("get_weather"));

    let stop_reason = events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    });
    assert_eq!(stop_reason, Some(StopReason::ToolUse));
}

#[tokio::test]
#[ignore = "hits live Ollama instance"]
async fn live_multi_turn_context() {
    let sf = ollama();

    // First turn
    let context = simple_context("My name is Zephyr.");
    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out on first turn");

    let reply: String = events
        .iter()
        .filter_map(|e| match e {
            AssistantMessageEvent::TextDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert!(!reply.is_empty(), "first turn should produce text");

    // Second turn with prior context
    let m = model();
    let context = AgentContext {
        system_prompt: "Reply in one short sentence.".into(),
        messages: vec![
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "My name is Zephyr.".into(),
                }],
                timestamp: 0,
                cache_hint: None,
            })),
            AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
                content: vec![ContentBlock::Text { text: reply }],
                provider: "ollama".into(),
                model_id: m.model_id,
                usage: Usage::default(),
                cost: Cost::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                error_kind: None,
                timestamp: 1,
                cache_hint: None,
            })),
            AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "What is my name?".into(),
                }],
                timestamp: 2,
                cache_hint: None,
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
        reply_lower.contains("zephyr"),
        "expected 'Zephyr' in multi-turn reply, got: {reply}"
    );
}

#[tokio::test]
#[ignore = "hits live Ollama instance"]
async fn live_stop_reason_mapping() {
    let sf = ollama();
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
#[ignore = "hits live Ollama instance"]
async fn live_model_not_found() {
    let sf = ollama();
    let context = simple_context("Hi.");

    let m = ModelSpec::new("ollama", "nonexistent-model-xyz-999");
    let options = StreamOptions::default();
    let token = CancellationToken::new();
    let stream = sf.stream(&m, &context, &options, token);
    let events: Vec<AssistantMessageEvent> = timeout(TIMEOUT, stream.collect::<Vec<_>>())
        .await
        .expect("timed out");

    let has_error = events
        .iter()
        .any(|e| matches!(e, AssistantMessageEvent::Error { .. }));
    assert!(
        has_error,
        "expected an Error event for nonexistent model, got: {events:?}"
    );
}
