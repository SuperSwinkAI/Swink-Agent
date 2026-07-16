//! Shared test helpers for eval integration tests.

pub mod judge_fixtures;

use std::time::Duration;

use swink_agent::{AssistantMessage, Cost, ModelSpec, StopReason, Usage};

use swink_agent_eval::{
    BudgetConstraints, EvalCase, ExpectedToolCall, Invocation, RecordedToolCall, ResponseCriteria,
    TurnRecord,
};

/// Build a minimal `Invocation` with the given tool call names and final response.
#[allow(dead_code)]
pub fn mock_invocation(
    tool_names: &[&str],
    final_response: Option<&str>,
    cost_total: f64,
    token_total: u64,
) -> Invocation {
    let tool_calls: Vec<RecordedToolCall> = tool_names
        .iter()
        .enumerate()
        .map(|(i, name)| RecordedToolCall::new(format!("call_{i}"), *name, serde_json::json!({})))
        .collect();

    let content = final_response.map_or_else(Vec::new, |text| {
        vec![swink_agent::ContentBlock::Text {
            text: text.to_string(),
        }]
    });

    let assistant_message = AssistantMessage::new(content, "test", "test-model")
        .with_usage(
            Usage::default()
                .with_input(token_total)
                .with_total(token_total),
        )
        .with_cost(Cost::default().with_total(cost_total))
        .with_stop_reason(StopReason::Stop)
        .with_timestamp(0);

    let mut invocation = Invocation::new(StopReason::Stop, ModelSpec::new("test", "test-model"))
        .with_turns(vec![
            TurnRecord::new(0, assistant_message)
                .with_tool_calls(tool_calls)
                .with_duration(Duration::from_millis(100)),
        ])
        .with_total_usage(
            Usage::default()
                .with_input(token_total)
                .with_total(token_total),
        )
        .with_total_cost(Cost::default().with_total(cost_total))
        .with_total_duration(Duration::from_millis(100));
    invocation.final_response = final_response.map(String::from);
    invocation
}

/// Build a minimal eval case with the given expected trajectory.
#[allow(dead_code)]
pub fn case_with_trajectory(expected: Vec<ExpectedToolCall>) -> EvalCase {
    EvalCase::new(
        "test-case",
        "Test Case",
        "You are a test agent.",
        vec!["test prompt".to_string()],
    )
    .with_expected_trajectory(expected)
}

/// Build a minimal eval case with response criteria.
#[allow(dead_code)]
pub fn case_with_response(criteria: ResponseCriteria) -> EvalCase {
    EvalCase::new(
        "test-case",
        "Test Case",
        "You are a test agent.",
        vec!["test prompt".to_string()],
    )
    .with_expected_response(criteria)
}

/// Build a multi-turn `Invocation` where each inner slice defines tool calls
/// for one turn, with custom arguments.
#[allow(dead_code)]
pub fn mock_invocation_multi_turn(turns: &[&[(&str, serde_json::Value)]]) -> Invocation {
    let turn_records: Vec<TurnRecord> = turns
        .iter()
        .enumerate()
        .map(|(i, calls)| {
            let tool_calls = calls
                .iter()
                .enumerate()
                .map(|(j, (name, args))| {
                    RecordedToolCall::new(format!("call_{i}_{j}"), *name, args.clone())
                })
                .collect();
            TurnRecord::new(
                i,
                AssistantMessage::new(vec![], "test", "test-model")
                    .with_stop_reason(StopReason::Stop)
                    .with_timestamp(0),
            )
            .with_tool_calls(tool_calls)
            .with_duration(Duration::from_millis(50))
        })
        .collect();

    Invocation::new(StopReason::Stop, ModelSpec::new("test", "test-model"))
        .with_turns(turn_records)
        .with_total_duration(Duration::from_millis(100))
}

/// Build a minimal eval case with budget constraints.
#[allow(dead_code)]
pub fn case_with_budget(budget: BudgetConstraints) -> EvalCase {
    EvalCase::new(
        "test-case",
        "Test Case",
        "You are a test agent.",
        vec!["test prompt".to_string()],
    )
    .with_budget(budget)
}

/// Build an eval case with both expected trajectory and response criteria.
#[allow(dead_code)]
pub fn case_with_trajectory_and_response(
    expected: Vec<ExpectedToolCall>,
    criteria: ResponseCriteria,
) -> EvalCase {
    EvalCase::new(
        "test-case",
        "Test Case",
        "You are a test agent.",
        vec!["test prompt".to_string()],
    )
    .with_expected_trajectory(expected)
    .with_expected_response(criteria)
}

/// Build a minimal `Invocation` with the given tool call names and a set response.
#[allow(dead_code)]
pub fn mock_invocation_with_response(tool_names: &[&str], response: &str) -> Invocation {
    mock_invocation(tool_names, Some(response), 0.01, 500)
}

/// Build a minimal `EvalCase` suitable for runner integration tests.
///
/// Spec 043 US2 helper — extracted so runner-extension tests share a stable
/// case shape without each re-declaring every field.
#[allow(dead_code)]
pub fn make_case(id: &str) -> EvalCase {
    EvalCase::new(
        id,
        format!("Case {id}"),
        "You are a test agent.",
        vec!["hello".to_string()],
    )
}
