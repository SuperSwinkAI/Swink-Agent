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
    let case = case_with_budget(
        BudgetConstraints::default()
            .with_max_cost(1.0)
            .with_max_input(1000)
            .with_max_turns(5),
    );
    let invocation = mock_invocation(&["read"], Some("done"), 0.5, 500);
    let result = BudgetEvaluator::new().evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

#[test]
fn fails_on_cost_exceeded() {
    let case = case_with_budget(BudgetConstraints::default().with_max_cost(0.001));
    let invocation = mock_invocation(&["read"], Some("done"), 0.01, 100);
    let result = BudgetEvaluator::new().evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
    assert!(result.details.unwrap().contains("cost"));
}

#[test]
fn fails_on_input_exceeded() {
    let case = case_with_budget(BudgetConstraints::default().with_max_input(50));
    let invocation = mock_invocation(&["read"], Some("done"), 0.01, 100);
    let result = BudgetEvaluator::new().evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
    assert!(result.details.unwrap().contains("input"));
}

#[test]
fn fails_on_output_exceeded() {
    let case = case_with_budget(BudgetConstraints::default().with_max_output(10));
    let invocation = Invocation::new(StopReason::Stop, ModelSpec::new("test", "test-model"))
        .with_total_usage(Usage::default().with_output(11).with_total(11))
        .with_total_duration(Duration::from_secs(1));
    let result = BudgetEvaluator::new().evaluate(&case, &invocation).unwrap();
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
            cost: Cost::default().with_total(0.005),
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
            cost: Cost::default().with_total(0.005),
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
    let case = case_with_budget(BudgetConstraints::default().with_max_cost(0.01));

    let result = runner.run_case(&case, &factory).await.unwrap();

    assert_eq!(result.invocation.turns.len(), 2);
    assert!(result.invocation.total_cost.total <= 0.01);
}

#[test]
fn budget_constraints_to_policies_none_when_unset() {
    let constraints = BudgetConstraints::default();
    let (budget_policy, max_turns_policy) = constraints.to_policies();

    assert!(budget_policy.is_none());
    assert!(max_turns_policy.is_none());
}
