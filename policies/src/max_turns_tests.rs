//! Tests for `max_turns`.
#![cfg(test)]

use super::*;
use swink_agent::{Cost, Usage};

fn make_ctx_at_turn<'a>(
    turn: usize,
    usage: &'a Usage,
    cost: &'a Cost,
    state: &'a swink_agent::SessionState,
) -> PolicyContext<'a> {
    PolicyContext::new(turn, usage, cost, 0, false, &[], state)
}

#[test]
fn stops_at_max() {
    let policy = MaxTurnsPolicy::new(5);
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx_at_turn(5, &usage, &cost, &state);
    assert!(matches!(
        PreTurnPolicy::evaluate(&policy, &ctx),
        PolicyVerdict::Stop(_)
    ));
}

#[test]
fn continues_below_max() {
    let policy = MaxTurnsPolicy::new(5);
    let usage = Usage::default();
    let cost = Cost::default();
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx_at_turn(4, &usage, &cost, &state);
    assert!(matches!(
        PreTurnPolicy::evaluate(&policy, &ctx),
        PolicyVerdict::Continue
    ));
}

#[test]
fn boundary_at_max() {
    let policy = MaxTurnsPolicy::new(3);
    let usage = Usage::default();
    let cost = Cost::default();

    // At turn 2 (0-indexed), still below max of 3
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx_at_turn(2, &usage, &cost, &state);
    assert!(matches!(
        PreTurnPolicy::evaluate(&policy, &ctx),
        PolicyVerdict::Continue
    ));

    // At turn 3, reaches max
    let state = swink_agent::SessionState::new();
    let ctx = make_ctx_at_turn(3, &usage, &cost, &state);
    assert!(matches!(
        PreTurnPolicy::evaluate(&policy, &ctx),
        PolicyVerdict::Stop(_)
    ));
}
