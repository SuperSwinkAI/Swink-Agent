//! Integration tests for budget evaluator and policy-backed budget enforcement.

mod common;

use std::sync::Arc;
use std::time::Duration;

use swink_agent::{
    Agent, AgentOptions, AssistantMessageEvent, Cost, ModelSpec, StopReason, Usage,
    testing::MockStreamFn,
};
use swink_agent_eval::{
    AgentFactory, BudgetConstraints, BudgetEvaluator, EvalCase, EvalError, EvalRunner, Evaluator,
    Invocation, Verdict,
};
use tokio_util::sync::CancellationToken;

use common::{case_with_budget, mock_invocation};

#[test]
fn passes_within_budget() {
    let case = case_with_budget(BudgetConstraints {
        max_cost: Some(1.0),
        max_input: Some(1000),
        max_output: None,
        max_turns: Some(5),
    });
    let invocation = mock_invocation(&["read"], Some("done"), 0.5, 500);
    let result = BudgetEvaluator.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

#[test]
fn fails_on_cost_exceeded() {
    let case = case_with_budget(BudgetConstraints {
        max_cost: Some(0.001),
        max_input: None,
        max_output: None,
        max_turns: None,
    });
    let invocation = mock_invocation(&["read"], Some("done"), 0.01, 100);
    let result = BudgetEvaluator.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
    assert!(result.details.unwrap().contains("cost"));
}

#[test]
fn fails_on_input_exceeded() {
    let case = case_with_budget(BudgetConstraints {
        max_cost: None,
        max_input: Some(50),
        max_output: None,
        max_turns: None,
    });
    let invocation = mock_invocation(&["read"], Some("done"), 0.01, 100);
    let result = BudgetEvaluator.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
    assert!(result.details.unwrap().contains("input"));
}

#[test]
fn fails_on_output_exceeded() {
    let case = case_with_budget(BudgetConstraints {
        max_cost: None,
        max_input: None,
        max_output: Some(10),
        max_turns: None,
    });
    let invocation = Invocation {
        turns: vec![],
        total_usage: Usage {
            output: 11,
            total: 11,
            ..Default::default()
        },
        total_cost: Cost::default(),
        total_duration: Duration::from_secs(1),
        final_response: None,
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "test-model"),
    };
    let result = BudgetEvaluator.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
    assert!(result.details.unwrap().contains("output"));
}

fn tool_response_events(id: &str) -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::ToolCallStart {
            content_index: 0,
            id: id.to_string(),
            name: "mock_tool".to_string(),
        },
        AssistantMessageEvent::ToolCallDelta {
            content_index: 0,
            delta: "{}".to_string(),
        },
        AssistantMessageEvent::ToolCallEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            cost: Cost {
                total: 0.005,
                ..Default::default()
            },
        },
    ]
}

fn text_response_events(text: &str) -> Vec<AssistantMessageEvent> {
    vec![
        AssistantMessageEvent::Start,
        AssistantMessageEvent::TextStart { content_index: 0 },
        AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: text.to_string(),
        },
        AssistantMessageEvent::TextEnd { content_index: 0 },
        AssistantMessageEvent::Done {
            stop_reason: StopReason::ToolUse,
            usage: Usage::default(),
            cost: Cost {
                total: 0.005,
                ..Default::default()
            },
        },
    ]
}

struct PolicyAwareFactory {
    responses: Vec<Vec<AssistantMessageEvent>>,
}

impl PolicyAwareFactory {
    fn new() -> Self {
        Self {
            responses: vec![
                tool_response_events("call-1"),
                text_response_events("after tool 1"),
                tool_response_events("call-2"),
                text_response_events("after tool 2"),
                tool_response_events("call-3"),
                text_response_events("after tool 3"),
            ],
        }
    }
}

impl AgentFactory for PolicyAwareFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let cancel = CancellationToken::new();
        let stream_fn = Arc::new(MockStreamFn::new(self.responses.clone()));
        let mut options = AgentOptions::new_simple(
            &case.system_prompt,
            ModelSpec::new("test", "test-model"),
            stream_fn,
        );

        if let Some(budget) = &case.budget {
            let (budget_policy, max_turns_policy) = budget.to_policies();
            if let Some(policy) = budget_policy {
                options = options.with_pre_turn_policy(policy);
            }
            if let Some(policy) = max_turns_policy {
                options = options.with_pre_turn_policy(policy);
            }
        }

        Ok((Agent::new(options), cancel))
    }
}

#[tokio::test]
async fn budget_policy_stops_multi_turn_run() {
    let factory = PolicyAwareFactory::new();
    let runner = EvalRunner::with_defaults();
    let case = case_with_budget(BudgetConstraints {
        max_cost: Some(0.01),
        max_input: None,
        max_output: None,
        max_turns: None,
    });

    let result = runner.run_case(&case, &factory).await.unwrap();

    assert_eq!(result.invocation.turns.len(), 2);
    assert!(result.invocation.total_cost.total <= 0.01);
}

#[test]
fn budget_constraints_to_policies_none_when_unset() {
    let constraints = BudgetConstraints {
        max_cost: None,
        max_input: None,
        max_output: None,
        max_turns: None,
    };
    let (budget_policy, max_turns_policy) = constraints.to_policies();

    assert!(budget_policy.is_none());
    assert!(max_turns_policy.is_none());
}
