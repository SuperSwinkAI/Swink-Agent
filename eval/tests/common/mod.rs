//! Shared test helpers for eval integration tests.

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
        .map(|(i, name)| RecordedToolCall {
            id: format!("call_{i}"),
            name: (*name).to_string(),
            arguments: serde_json::json!({}),
        })
        .collect();

    let content = final_response.map_or_else(Vec::new, |text| {
        vec![swink_agent::ContentBlock::Text {
            text: text.to_string(),
        }]
    });

    let assistant_message = AssistantMessage {
        content,
        provider: "test".to_string(),
        model_id: "test-model".to_string(),
        usage: Usage {
            input: token_total,
            total: token_total,
            ..Default::default()
        },
        cost: Cost {
            total: cost_total,
            ..Default::default()
        },
        stop_reason: StopReason::Stop,
        error_message: None,
        error_kind: None,
        timestamp: 0,
        cache_hint: None,
    };

    Invocation {
        turns: vec![TurnRecord {
            turn_index: 0,
            assistant_message,
            tool_calls,
            tool_results: vec![],
            duration: Duration::from_millis(100),
        }],
        total_usage: Usage {
            input: token_total,
            total: token_total,
            ..Default::default()
        },
        total_cost: Cost {
            total: cost_total,
            ..Default::default()
        },
        total_duration: Duration::from_millis(100),
        final_response: final_response.map(String::from),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "test-model"),
    }
}

/// Build a minimal eval case with the given expected trajectory.
#[allow(dead_code)]
pub fn case_with_trajectory(expected: Vec<ExpectedToolCall>) -> EvalCase {
    EvalCase {
        id: "test-case".to_string(),
        name: "Test Case".to_string(),
        description: None,
        system_prompt: "You are a test agent.".to_string(),
        user_messages: vec!["test prompt".to_string()],
        expected_trajectory: Some(expected),
        expected_response: None,
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

/// Build a minimal eval case with response criteria.
#[allow(dead_code)]
pub fn case_with_response(criteria: ResponseCriteria) -> EvalCase {
    EvalCase {
        id: "test-case".to_string(),
        name: "Test Case".to_string(),
        description: None,
        system_prompt: "You are a test agent.".to_string(),
        user_messages: vec!["test prompt".to_string()],
        expected_trajectory: None,
        expected_response: Some(criteria),
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
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
                .map(|(j, (name, args))| RecordedToolCall {
                    id: format!("call_{i}_{j}"),
                    name: (*name).to_string(),
                    arguments: args.clone(),
                })
                .collect();
            TurnRecord {
                turn_index: i,
                assistant_message: AssistantMessage {
                    content: vec![],
                    provider: "test".to_string(),
                    model_id: "test-model".to_string(),
                    usage: Usage::default(),
                    cost: Cost::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    error_kind: None,
                    timestamp: 0,
                    cache_hint: None,
                },
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

/// Build a minimal eval case with budget constraints.
#[allow(dead_code)]
pub fn case_with_budget(budget: BudgetConstraints) -> EvalCase {
    EvalCase {
        id: "test-case".to_string(),
        name: "Test Case".to_string(),
        description: None,
        system_prompt: "You are a test agent.".to_string(),
        user_messages: vec!["test prompt".to_string()],
        expected_trajectory: None,
        expected_response: None,
        budget: Some(budget),
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

/// Build an eval case with both expected trajectory and response criteria.
#[allow(dead_code)]
pub fn case_with_trajectory_and_response(
    expected: Vec<ExpectedToolCall>,
    criteria: ResponseCriteria,
) -> EvalCase {
    EvalCase {
        id: "test-case".to_string(),
        name: "Test Case".to_string(),
        description: None,
        system_prompt: "You are a test agent.".to_string(),
        user_messages: vec!["test prompt".to_string()],
        expected_trajectory: Some(expected),
        expected_response: Some(criteria),
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

/// Build a minimal `Invocation` with the given tool call names and a set response.
#[allow(dead_code)]
pub fn mock_invocation_with_response(tool_names: &[&str], response: &str) -> Invocation {
    mock_invocation(tool_names, Some(response), 0.01, 500)
}
