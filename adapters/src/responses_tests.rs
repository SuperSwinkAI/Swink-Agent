//! Tests for `responses`.
#![cfg(test)]

use super::*;
use swink_agent::{AgentMessage, LlmMessage};

fn spec(level: ThinkingLevel) -> ModelSpec {
    ModelSpec::new("openai", "gpt-5.6-luna").with_thinking_level(level)
}

#[test]
fn request_always_sends_store_false_and_non_empty_instructions() {
    let context = AgentContext::new("", Vec::new(), Vec::new());
    let body = serde_json::to_value(build_request(
        &spec(ThinkingLevel::Off),
        &context,
        &StreamOptions::default(),
    ))
    .unwrap();
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["instructions"], DEFAULT_INSTRUCTIONS);
    assert!(
        body.get("reasoning").is_none(),
        "Off must not send reasoning"
    );

    let context = AgentContext::new("Be terse.", Vec::new(), Vec::new());
    let body = serde_json::to_value(build_request(
        &spec(ThinkingLevel::High),
        &context,
        &StreamOptions::default(),
    ))
    .unwrap();
    assert_eq!(body["instructions"], "Be terse.");
    assert_eq!(body["reasoning"]["effort"], "high");
    // The system prompt is `instructions`, never an input item.
    assert!(body["input"].as_array().unwrap().is_empty());
}

#[test]
fn per_request_reasoning_effort_overrides_the_model_level() {
    let context = AgentContext::new("", Vec::new(), Vec::new());
    let max = StreamOptions::default().with_serving(
        swink_agent::ServingOptions::default().with_reasoning_effort(ReasoningEffort::Max),
    );
    let body =
        serde_json::to_value(build_request(&spec(ThinkingLevel::Low), &context, &max)).unwrap();
    assert_eq!(body["reasoning"]["effort"], "max");
    let off = StreamOptions::default().with_serving(
        swink_agent::ServingOptions::default().with_reasoning_effort(ReasoningEffort::Off),
    );
    let body =
        serde_json::to_value(build_request(&spec(ThinkingLevel::High), &context, &off)).unwrap();
    assert!(
        body.get("reasoning").is_none(),
        "per-request Off silences the model level"
    );
}

#[test]
fn reasoning_effort_covers_every_level() {
    assert_eq!(reasoning_effort(ThinkingLevel::Minimal), Some("minimal"));
    assert_eq!(reasoning_effort(ThinkingLevel::Low), Some("low"));
    assert_eq!(reasoning_effort(ThinkingLevel::Medium), Some("medium"));
    assert_eq!(reasoning_effort(ThinkingLevel::High), Some("high"));
    assert_eq!(reasoning_effort(ThinkingLevel::ExtraHigh), Some("xhigh"));
    assert_eq!(reasoning_effort(ThinkingLevel::Off), None);
}

#[test]
fn tool_round_trip_converts_to_function_call_and_output_items() {
    let assistant = AssistantMessage::new(
        vec![
            ContentBlock::Text {
                text: "Checking.".to_owned(),
            },
            ContentBlock::ToolCall {
                id: "call_1".to_owned(),
                name: "weather".to_owned(),
                arguments: serde_json::json!({"city": "Oslo"}),
                partial_json: None,
            },
        ],
        "openai",
        "gpt-5.6-luna",
    );
    let result = ToolResultMessage::new(
        "call_1",
        vec![ContentBlock::Text {
            text: "12C".to_owned(),
        }],
    );
    let user = UserMessage::new(vec![ContentBlock::Text {
        text: "What next?".to_owned(),
    }]);
    let messages = vec![
        AgentMessage::Llm(LlmMessage::User(UserMessage::new(vec![
            ContentBlock::Text {
                text: "Weather in Oslo?".to_owned(),
            },
        ]))),
        AgentMessage::Llm(LlmMessage::Assistant(assistant)),
        AgentMessage::Llm(LlmMessage::ToolResult(result)),
        AgentMessage::Llm(LlmMessage::User(user)),
    ];
    let context = AgentContext::new("sys", messages, Vec::new());
    let body = serde_json::to_value(build_request(
        &spec(ThinkingLevel::Off),
        &context,
        &StreamOptions::default(),
    ))
    .unwrap();
    let input = body["input"].as_array().unwrap();
    assert_eq!(input.len(), 5, "{input:#?}");
    assert_eq!(input[0]["type"], "message");
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[0]["content"][0]["type"], "input_text");
    assert_eq!(input[1]["type"], "message");
    assert_eq!(input[1]["role"], "assistant");
    assert_eq!(input[1]["content"][0]["type"], "output_text");
    assert_eq!(input[1]["content"][0]["text"], "Checking.");
    assert_eq!(input[2]["type"], "function_call");
    assert_eq!(input[2]["call_id"], "call_1");
    assert_eq!(input[2]["name"], "weather");
    assert_eq!(input[2]["arguments"], r#"{"city":"Oslo"}"#);
    assert_eq!(input[3]["type"], "function_call_output");
    assert_eq!(input[3]["call_id"], "call_1");
    assert_eq!(input[3]["output"], "12C");
    assert_eq!(input[4]["role"], "user");
}

#[test]
fn text_format_maps_json_and_schema() {
    let json = StreamOptions::default()
        .with_serving(swink_agent::ServingOptions::default().with_format(ResponseFormat::Json));
    assert_eq!(text_format(&json).unwrap()["format"]["type"], "json_object");
    let schema =
        StreamOptions::default().with_serving(swink_agent::ServingOptions::default().with_format(
            ResponseFormat::Schema(serde_json::json!({"type": "object"})),
        ));
    let value = text_format(&schema).unwrap();
    assert_eq!(value["format"]["type"], "json_schema");
    assert_eq!(value["format"]["strict"], true);
    assert_eq!(value["format"]["schema"]["type"], "object");
}

#[test]
fn usage_splits_cached_tokens_out_of_input() {
    let usage: ResponsesUsage = serde_json::from_value(serde_json::json!({
        "input_tokens": 1000,
        "output_tokens": 50,
        "total_tokens": 1050,
        "input_tokens_details": {"cached_tokens": 600},
        "output_tokens_details": {"reasoning_tokens": 20}
    }))
    .unwrap();
    let usage = usage.to_usage();
    assert_eq!(usage.input, 400);
    assert_eq!(usage.cache_read, 600);
    assert_eq!(usage.output, 50);
    assert_eq!(usage.total, 1050);
    assert_eq!(usage.extra["output_tokens_details.reasoning_tokens"], 20);
    assert_eq!(usage.extra["input_tokens_details.cached_tokens"], 600);
}

#[test]
fn trailing_slash_stripped() {
    let f = ResponsesStreamFn::new("https://api.openai.com/", "k");
    assert_eq!(f.shell.base_url(), "https://api.openai.com");
    assert_eq!(f.shell.url(), "https://api.openai.com/v1/responses");
    let f = f.with_responses_path("/responses");
    assert_eq!(f.shell.url(), "https://api.openai.com/responses");
}
