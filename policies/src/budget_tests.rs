//! Tests for `budget`.
#![cfg(test)]

use super::*;
use swink_agent::{Cost, Usage};

fn make_ctx<'a>(
    usage: &'a Usage,
    cost: &'a Cost,
    state: &'a swink_agent::SessionState,
) -> PolicyContext<'a> {
    PolicyContext::new(0, usage, cost, 0, false, &[], state)
}

#[test]
fn name_returns_budget() {
    assert_eq!(BudgetPolicy::new().name(), "budget");
}

#[test]
fn no_limits_returns_continue() {
    let policy = BudgetPolicy::new();
    let usage = Usage::default().with_input(1000).with_output(500);
    let cost = Cost::default().with_total(10.0);
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx(&usage, &cost, &state);
    assert!(matches!(policy.evaluate(&ctx), PolicyVerdict::Continue));
}

#[test]
fn cost_exceeded_returns_stop() {
    let policy = BudgetPolicy::new().with_max_cost(1.0);
    let usage = Usage::default();
    let cost = Cost::default().with_total(1.5);
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx(&usage, &cost, &state);
    assert!(matches!(policy.evaluate(&ctx), PolicyVerdict::Stop(_)));
}

#[test]
fn cost_not_exceeded_returns_continue() {
    let policy = BudgetPolicy::new().with_max_cost(5.0);
    let usage = Usage::default();
    let cost = Cost::default().with_total(4.99);
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx(&usage, &cost, &state);
    assert!(matches!(policy.evaluate(&ctx), PolicyVerdict::Continue));
}

#[test]
fn token_exceeded_returns_stop() {
    let policy = BudgetPolicy::new().with_max_input(100);
    let usage = Usage::default().with_input(150);
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx(&usage, &cost, &state);
    assert!(matches!(policy.evaluate(&ctx), PolicyVerdict::Stop(_)));
}

#[test]
fn boundary_value_at_limit() {
    let policy = BudgetPolicy::new().with_max_cost(1.0);
    let usage = Usage::default();
    let cost = Cost::default().with_total(1.0);
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx(&usage, &cost, &state);
    // At exactly the limit, should stop (>= comparison)
    assert!(matches!(policy.evaluate(&ctx), PolicyVerdict::Stop(_)));
}
