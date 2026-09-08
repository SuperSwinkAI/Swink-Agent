//! Tests for `types`.
#![cfg(test)]

use super::*;
use swink_agent::{Cost, PolicyContext, PolicyVerdict, PreTurnPolicy, SessionState, Usage};

fn make_ctx<'a>(turn_index: usize, usage: &'a Usage, cost: &'a Cost) -> PolicyContext<'a> {
    let state = Box::leak(Box::new(SessionState::new()));
    PolicyContext::new(turn_index, usage, cost, 0, false, &[], state)
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

#[test]
fn budget_constraints_to_policies_builds_budget_only_for_cost() {
    let constraints = BudgetConstraints {
        max_cost: Some(1.0),
        max_input: None,
        max_output: None,
        max_turns: None,
    };

    let (budget_policy, max_turns_policy) = constraints.to_policies();
    let usage = Usage::default();
    let cost = Cost::default().with_total(1.0);
    let ctx = make_ctx(0, &usage, &cost);

    assert!(matches!(
        PreTurnPolicy::evaluate(&budget_policy.unwrap(), &ctx),
        PolicyVerdict::Stop(_)
    ));
    assert!(max_turns_policy.is_none());
}

#[test]
fn budget_constraints_to_policies_builds_budget_only_for_input_output() {
    let constraints = BudgetConstraints {
        max_cost: None,
        max_input: Some(10),
        max_output: Some(20),
        max_turns: None,
    };

    let (budget_policy, max_turns_policy) = constraints.to_policies();
    let usage = Usage::default()
        .with_input(10)
        .with_output(20)
        .with_total(30);
    let cost = Cost::default();
    let ctx = make_ctx(0, &usage, &cost);

    assert!(matches!(
        PreTurnPolicy::evaluate(&budget_policy.unwrap(), &ctx),
        PolicyVerdict::Stop(_)
    ));
    assert!(max_turns_policy.is_none());
}

#[test]
fn budget_constraints_to_policies_builds_both_policies_when_needed() {
    let constraints = BudgetConstraints {
        max_cost: Some(2.0),
        max_input: None,
        max_output: None,
        max_turns: Some(3),
    };

    let (budget_policy, max_turns_policy) = constraints.to_policies();
    let usage = Usage::default();
    let cost = Cost::default().with_total(2.0);
    let budget_ctx = make_ctx(0, &usage, &cost);
    let turn_cost = Cost::default();
    let turn_ctx = make_ctx(3, &usage, &turn_cost);

    assert!(matches!(
        PreTurnPolicy::evaluate(&budget_policy.unwrap(), &budget_ctx),
        PolicyVerdict::Stop(_)
    ));
    assert!(matches!(
        PreTurnPolicy::evaluate(&max_turns_policy.unwrap(), &turn_ctx),
        PolicyVerdict::Stop(_)
    ));
}
