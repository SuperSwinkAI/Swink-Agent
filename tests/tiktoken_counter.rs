#![cfg(feature = "tiktoken")]

use swink_agent::{
    AgentMessage, AssistantMessage, ContentBlock, Cost, LlmMessage, StopReason, TiktokenCounter,
    TokenCounter, Usage, UserMessage,
};

fn user_text_message(text: &str) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        timestamp: 0,
        cache_hint: None,
    }))
}

fn assistant_tool_call_message(name: &str, arguments: serde_json::Value) -> AgentMessage {
    AgentMessage::Llm(LlmMessage::Assistant(AssistantMessage {
        content: vec![ContentBlock::ToolCall {
            id: "tool-1".into(),
            name: name.into(),
            arguments,
            partial_json: None,
        }],
        provider: "test".into(),
        model_id: "test-model".into(),
        usage: Usage::default(),
        cost: Cost::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    }))
}

#[test]
fn cl100k_counts_text_with_tiktoken() {
    let counter = TiktokenCounter::cl100k().unwrap();
    let message = user_text_message("Swink Agent keeps context budgets honest.");
    let expected = tiktoken_rs::cl100k_base()
        .unwrap()
        .encode_with_special_tokens("Swink Agent keeps context budgets honest.")
        .len();

    assert_eq!(counter.count_tokens(&message), expected);
}

#[test]
fn tool_call_count_includes_name_and_arguments() {
    let counter = TiktokenCounter::cl100k().unwrap();
    let message = assistant_tool_call_message("lookup_docs", serde_json::json!({"topic": "mcp"}));
    let bpe = tiktoken_rs::cl100k_base().unwrap();
    let expected = bpe.encode_with_special_tokens("lookup_docs").len()
        + bpe.encode_with_special_tokens("{\"topic\":\"mcp\"}").len();

    assert_eq!(counter.count_tokens(&message), expected);
}

#[test]
fn custom_messages_keep_flat_100_token_estimate() {
    #[derive(Debug)]
    struct TestCustom;

    impl swink_agent::CustomMessage for TestCustom {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    let counter = TiktokenCounter::cl100k().unwrap();
    let message = AgentMessage::Custom(Box::new(TestCustom));

    assert_eq!(counter.count_tokens(&message), 100);
}

#[test]
fn from_model_builds_counter_for_known_model() {
    let counter = TiktokenCounter::from_model("gpt-4o").unwrap();
    let message = user_text_message("hello");

    assert!(counter.count_tokens(&message) > 0);
}
