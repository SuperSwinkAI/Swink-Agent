//! Tests for `response`.
#![cfg(test)]

use super::*;

use std::sync::Arc;
use std::time::Duration;

use swink_agent::{AssistantMessage, ContentBlock, Cost, ModelSpec, StopReason, Usage};

use crate::types::{EvalCase, Invocation, TurnRecord};

fn minimal_case_with_response(criteria: ResponseCriteria) -> EvalCase {
    EvalCase {
        id: "test".to_string(),
        name: "Test".to_string(),
        description: None,
        system_prompt: "test".to_string(),
        user_messages: vec!["test".to_string()],
        expected_trajectory: None,
        expected_response: Some(criteria),
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

fn invocation_with_response(text: &str) -> Invocation {
    Invocation {
        turns: vec![TurnRecord {
            turn_index: 0,
            assistant_message: AssistantMessage::new(
                vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
                "test".to_string(),
                "test-model".to_string(),
            )
            .with_timestamp(0),
            tool_calls: vec![],
            tool_results: vec![],
            duration: Duration::from_millis(10),
        }],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(10),
        final_response: Some(text.to_string()),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "test-model"),
    }
}

#[test]
fn truncate_short_string() {
    assert_eq!(truncate("hello", 10), "hello");
}

#[test]
fn truncate_long_string() {
    let long = "a".repeat(200);
    let result = truncate(&long, 100);
    assert_eq!(result.len(), 103); // 100 + "..."
    assert!(result.ends_with("..."));
}

#[test]
fn truncate_multibyte_string_is_utf8_safe() {
    let text = format!("{}🙂tail", "a".repeat(99));
    let result = truncate(&text, 100);
    assert_eq!(result, format!("{}🙂...", "a".repeat(99)));
}

#[test]
fn custom_fn_panic_caught_as_failure() {
    let criteria = ResponseCriteria::Custom(Arc::new(|_: &str| -> Score {
        panic!("deliberate test panic");
    }));
    let case = minimal_case_with_response(criteria);
    let invocation = invocation_with_response("anything");

    let result = ResponseMatcher.evaluate(&case, &invocation).unwrap();
    assert!((result.score.value - 0.0).abs() < f64::EPSILON);
    let details = result.details.unwrap();
    assert!(
        details.contains("panicked"),
        "expected panic mention, got: {details}"
    );
    assert!(
        details.contains("deliberate test panic"),
        "expected panic message, got: {details}"
    );
}
