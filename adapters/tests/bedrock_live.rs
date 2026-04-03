#![cfg(feature = "bedrock")]
//! Live API tests for `BedrockStreamFn`.
//!
//! These tests hit the real AWS Bedrock API and are skipped by default.
//! Run with: `cargo test -p swink-agent-adapters --test bedrock_live -- --ignored`
//! Requires `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, and `AWS_REGION`
//! in `.env` or environment.

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
use swink_agent_adapters::BedrockStreamFn;

// ── Constants ────────────────────────────────────────────────────────────────

const TIMEOUT: Duration = Duration::from_secs(30);

// ── Helpers ──────────────────────────────────────────────────────────────────

fn aws_creds() -> (String, String, String, Option<String>) {
    dotenvy::dotenv().ok();
    let access_key =
        std::env::var("AWS_ACCESS_KEY_ID").expect("AWS_ACCESS_KEY_ID must be set for live tests");
    let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY")
        .expect("AWS_SECRET_ACCESS_KEY must be set for live tests");
    let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
    (access_key, secret_key, region, session_token)
}

fn cheap_model() -> ModelSpec {
    dotenvy::dotenv().ok();
    let model_id = std::env::var("BEDROCK_MODEL")
        .unwrap_or_else(|_| "anthropic.claude-3-5-haiku-20241022-v1:0".to_string());
    ModelSpec::new("bedrock", &model_id)
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
    sf: &BedrockStreamFn,
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

// ── Live Tests ───────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires AWS credentials"]
async fn live_text_stream() {
    let (access_key, secret_key, region, session_token) = aws_creds();
    let sf = BedrockStreamFn::new(&region, &access_key, &secret_key, session_token);
    let context = simple_context("What color is the sky?");

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out");

    let names: Vec<&str> = events.iter().map(event_name).collect();
    println!("events: {names:?}");

    assert!(names.contains(&"Start"), "missing Start event");
    assert!(names.contains(&"TextStart"), "missing TextStart event");
    assert!(names.contains(&"TextDelta"), "missing TextDelta event");
    assert!(names.contains(&"TextEnd"), "missing TextEnd event");
    assert!(names.contains(&"Done"), "missing Done event");

    // Assembled text should be non-empty
    let text: String = events
        .iter()
        .filter_map(|e| {
            if let AssistantMessageEvent::TextDelta { delta, .. } = e {
                Some(delta.as_str())
            } else {
                None
            }
        })
        .collect();
    assert!(!text.is_empty(), "assembled text should be non-empty");
}

#[tokio::test]
#[ignore = "requires AWS credentials"]
async fn live_usage_and_cost() {
    let (access_key, secret_key, region, session_token) = aws_creds();
    let sf = BedrockStreamFn::new(&region, &access_key, &secret_key, session_token);
    let context = simple_context("Say hello.");

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out");

    let done = events
        .iter()
        .find(|e| matches!(e, AssistantMessageEvent::Done { .. }));
    assert!(done.is_some(), "missing Done event");
    if let Some(AssistantMessageEvent::Done { usage, .. }) = done {
        assert!(usage.input > 0, "input tokens should be non-zero");
        assert!(usage.output > 0, "output tokens should be non-zero");
    }
}

#[tokio::test]
#[ignore = "requires AWS credentials"]
async fn live_invalid_creds_returns_auth_error() {
    let sf = BedrockStreamFn::new("us-east-1", "BOGUS_KEY", "BOGUS_SECRET", None);
    let context = simple_context("Hello");

    let events = timeout(TIMEOUT, collect_events(&sf, &context))
        .await
        .expect("timed out");

    let has_error = events
        .iter()
        .any(|e| matches!(e, AssistantMessageEvent::Error { .. }));
    assert!(has_error, "expected error event for invalid credentials");
}
