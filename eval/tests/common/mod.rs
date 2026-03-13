//! Shared test helpers for eval integration tests.

use std::time::Duration;

use swink_agent::{AssistantMessage, Cost, ModelSpec, StopReason, Usage};

use swink_agent_eval::{
    BudgetConstraints, EvalCase, ExpectedToolCall, Invocation, RecordedToolCall, ResponseCriteria,
    TurnRecord,
};

/// Build a minimal `Invocation` with the given tool call names and final response.
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
            total: token_total,
            ..Default::default()
        },
        cost: Cost {
            total: cost_total,
            ..Default::default()
        },
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
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
    }
}
