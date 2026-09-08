//! Tests for `gate`.
#![cfg(test)]

use super::*;

use swink_agent::{Cost, Usage};

use crate::types::{EvalSetResult, EvalSummary};

fn make_result(passed: usize, failed: usize, cost: f64, duration: Duration) -> EvalSetResult {
    EvalSetResult {
        eval_set_id: "test".to_string(),
        case_results: Vec::new(),
        summary: EvalSummary {
            total_cases: passed + failed,
            passed,
            failed,
            total_cost: Cost::default().with_total(cost),
            total_usage: Usage::default(),
            total_duration: duration,
        },
        timestamp: 0,
    }
}

#[test]
fn all_pass_no_config() {
    let result = make_result(5, 2, 1.0, Duration::from_secs(10));
    let config = GateConfig::new();
    let gate = check_gate(&result, &config);
    assert!(gate.passed);
    assert_eq!(gate.exit_code, 0);
}

#[test]
fn pass_rate_met() {
    let result = make_result(9, 1, 0.5, Duration::from_secs(5));
    let config = GateConfig::new().with_min_pass_rate(0.9);
    let gate = check_gate(&result, &config);
    assert!(gate.passed);
}

#[test]
fn pass_rate_not_met() {
    let result = make_result(8, 2, 0.5, Duration::from_secs(5));
    let config = GateConfig::new().with_min_pass_rate(0.9);
    let gate = check_gate(&result, &config);
    assert!(!gate.passed);
    assert_eq!(gate.exit_code, 1);
    assert!(gate.summary.contains("pass rate"));
}

#[test]
fn cost_exceeded() {
    let result = make_result(10, 0, 5.0, Duration::from_secs(5));
    let config = GateConfig::new().with_max_cost(2.0);
    let gate = check_gate(&result, &config);
    assert!(!gate.passed);
    assert!(gate.summary.contains("cost"));
}

#[test]
fn duration_exceeded() {
    let result = make_result(10, 0, 0.5, Duration::from_mins(1));
    let config = GateConfig::new().with_max_duration(Duration::from_secs(30));
    let gate = check_gate(&result, &config);
    assert!(!gate.passed);
    assert!(gate.summary.contains("duration"));
}

#[test]
fn multiple_failures_reported() {
    let result = make_result(5, 5, 10.0, Duration::from_secs(5));
    let config = GateConfig::new().with_min_pass_rate(0.9).with_max_cost(1.0);
    let gate = check_gate(&result, &config);
    assert!(!gate.passed);
    assert!(gate.summary.contains("pass rate"));
    assert!(gate.summary.contains("cost"));
}

#[test]
fn zero_cases_passes() {
    let result = make_result(0, 0, 0.0, Duration::from_secs(0));
    let config = GateConfig::new().with_min_pass_rate(0.95);
    let gate = check_gate(&result, &config);
    assert!(gate.passed);
}
