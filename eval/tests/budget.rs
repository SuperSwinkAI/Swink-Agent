//! Integration tests for budget evaluator.

mod common;

use std::time::Duration;

use swink_agent_eval::{BudgetConstraints, BudgetEvaluator, Evaluator, Verdict};

use common::{case_with_budget, mock_invocation};

#[test]
fn passes_within_budget() {
    let case = case_with_budget(BudgetConstraints {
        max_cost: Some(1.0),
        max_tokens: Some(1000),
        max_turns: Some(5),
        max_duration: Some(Duration::from_secs(10)),
    });
    let invocation = mock_invocation(&["read"], Some("done"), 0.5, 500);
    let result = BudgetEvaluator.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Pass);
}

#[test]
fn fails_on_cost_exceeded() {
    let case = case_with_budget(BudgetConstraints {
        max_cost: Some(0.001),
        max_tokens: None,
        max_turns: None,
        max_duration: None,
    });
    let invocation = mock_invocation(&["read"], Some("done"), 0.01, 100);
    let result = BudgetEvaluator.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
    assert!(result.details.unwrap().contains("cost"));
}

#[test]
fn fails_on_token_exceeded() {
    let case = case_with_budget(BudgetConstraints {
        max_cost: None,
        max_tokens: Some(50),
        max_turns: None,
        max_duration: None,
    });
    let invocation = mock_invocation(&["read"], Some("done"), 0.01, 100);
    let result = BudgetEvaluator.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
    assert!(result.details.unwrap().contains("tokens"));
}
