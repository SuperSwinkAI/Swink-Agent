//! Tests for `efficiency`.
#![cfg(test)]

use super::*;
use crate::types::{BudgetConstraints, RecordedToolCall, TurnRecord};
use std::time::Duration;
use swink_agent::{AssistantMessage, Cost, ModelSpec, StopReason, Usage};

fn make_invocation(turns: &[&[(&str, serde_json::Value)]]) -> Invocation {
    let turn_records: Vec<TurnRecord> = turns
        .iter()
        .enumerate()
        .map(|(i, calls)| {
            let tool_calls = calls
                .iter()
                .enumerate()
                .map(|(j, (name, args))| RecordedToolCall {
                    id: format!("call_{i}_{j}"),
                    name: (*name).to_string(),
                    arguments: args.clone(),
                })
                .collect();
            TurnRecord {
                turn_index: i,
                assistant_message: AssistantMessage::new(vec![], "test", "test-model")
                    .with_timestamp(0),
                tool_calls,
                tool_results: vec![],
                duration: Duration::from_millis(50),
            }
        })
        .collect();

    Invocation {
        turns: turn_records,
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(100),
        final_response: None,
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "test-model"),
    }
}

fn minimal_case() -> EvalCase {
    EvalCase {
        id: "test".to_string(),
        name: "Test".to_string(),
        description: None,
        system_prompt: "test".to_string(),
        user_messages: vec!["test".to_string()],
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
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

#[test]
fn no_tool_calls_returns_none() {
    let eval = EfficiencyEvaluator::new();
    let invocation = make_invocation(&[&[]]);
    assert!(eval.evaluate(&minimal_case(), &invocation).is_none());
}

#[test]
fn all_unique_perfect_score() {
    let eval = EfficiencyEvaluator::new();
    let invocation = make_invocation(&[&[
        ("read", serde_json::json!({"file": "a.rs"})),
        ("write", serde_json::json!({"file": "b.rs"})),
    ]]);
    // 1 turn, 2 unique calls out of 2 → dup_ratio=1.0
    // ideal = max(1, 2) = 2, actual = 1 → step_ratio = min(2,1)/1 = 1.0
    // composite = 0.6*1.0 + 0.4*1.0 = 1.0
    let result = eval.evaluate(&minimal_case(), &invocation).unwrap();
    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
}

#[test]
fn duplicate_calls_penalized() {
    let eval = EfficiencyEvaluator::new();
    let invocation = make_invocation(&[&[
        ("read", serde_json::json!({"file": "a.rs"})),
        ("read", serde_json::json!({"file": "a.rs"})),
        ("read", serde_json::json!({"file": "a.rs"})),
        ("write", serde_json::json!({"file": "b.rs"})),
    ]]);
    // 4 total, 2 unique → dup_ratio = 0.5
    // ideal = max(1, 2) = 2, actual = 1 → step_ratio = 1.0
    // composite = 0.6*0.5 + 0.4*1.0 = 0.7
    let result = eval.evaluate(&minimal_case(), &invocation).unwrap();
    assert!((result.score.value - 0.7).abs() < 0.01);
}

#[test]
fn step_ratio_uses_budget() {
    let eval = EfficiencyEvaluator::new();
    let invocation = make_invocation(&[
        &[("read", serde_json::json!({}))],
        &[("write", serde_json::json!({}))],
        &[("read", serde_json::json!({"file": "c.rs"}))],
        &[("write", serde_json::json!({"file": "d.rs"}))],
    ]);
    let mut case = minimal_case();
    case.budget = Some(BudgetConstraints {
        max_cost: None,
        max_input: None,
        max_output: None,
        max_turns: Some(2),
    });
    // 4 unique / 4 total → dup_ratio = 1.0
    // ideal = budget.max_turns = 2, actual = 4 → step_ratio = 2/4 = 0.5
    // composite = 0.6*1.0 + 0.4*0.5 = 0.8
    let result = eval.evaluate(&case, &invocation).unwrap();
    assert!((result.score.value - 0.8).abs() < 0.01);
}

#[test]
fn composite_weighted() {
    let eval = EfficiencyEvaluator::new();
    // 2 calls same args = 1 unique / 2 total → dup = 0.5
    // 2 turns, ideal = max(1,1) = 1, step = 1/2 = 0.5
    // composite = 0.6*0.5 + 0.4*0.5 = 0.5
    let invocation = make_invocation(&[
        &[("read", serde_json::json!({"file": "a.rs"}))],
        &[("read", serde_json::json!({"file": "a.rs"}))],
    ]);
    let result = eval.evaluate(&minimal_case(), &invocation).unwrap();
    assert!((result.score.value - 0.5).abs() < 0.01);
}
