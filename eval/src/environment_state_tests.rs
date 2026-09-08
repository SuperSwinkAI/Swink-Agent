//! Tests for `environment_state`.
#![cfg(test)]

use std::sync::Arc;

use swink_agent::{Cost, ModelSpec, StopReason, Usage};

use super::*;
use crate::types::{EnvironmentState, TurnRecord};

fn case_with_state_capture() -> EvalCase {
    EvalCase {
        id: "env".into(),
        name: "Environment".into(),
        description: None,
        system_prompt: "test".into(),
        user_messages: vec!["hi".into()],
        expected_trajectory: None,
        expected_response: None,
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: Some(vec![EnvironmentState {
            name: "created_file".into(),
            state: serde_json::json!("out.md"),
        }]),
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: Some(Arc::new(|_| {
            vec![EnvironmentState {
                name: "created_file".into(),
                state: serde_json::json!("out.md"),
            }]
        })),
    }
}

fn empty_invocation() -> Invocation {
    Invocation {
        turns: vec![TurnRecord {
            turn_index: 0,
            assistant_message: swink_agent::AssistantMessage::new(vec![], "test", "test-model")
                .with_timestamp(0),
            tool_calls: vec![],
            tool_results: vec![],
            duration: std::time::Duration::from_millis(10),
        }],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: std::time::Duration::from_millis(10),
        final_response: None,
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "test-model"),
    }
}

#[test]
fn returns_none_without_expected_states() {
    let mut case = case_with_state_capture();
    case.expected_environment_state = None;
    assert!(
        EnvironmentStateEvaluator
            .evaluate(&case, &empty_invocation())
            .is_none()
    );
}

#[test]
fn returns_none_without_state_capture() {
    let mut case = case_with_state_capture();
    case.state_capture = None;
    assert!(
        EnvironmentStateEvaluator
            .evaluate(&case, &empty_invocation())
            .is_none()
    );
}

#[test]
fn matching_state_passes() {
    let result = EnvironmentStateEvaluator
        .evaluate(&case_with_state_capture(), &empty_invocation())
        .expect("evaluator should apply");

    assert!(result.score.verdict().is_pass());
    assert!(
        result
            .details
            .as_deref()
            .is_some_and(|details| details.contains("created_file"))
    );
}
