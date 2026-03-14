mod common;

use std::time::Duration;

use swink_agent::{Cost, Usage};

use swink_agent_eval::{
    EvalCaseResult, EvalSetResult, EvalSummary, GateConfig, Verdict, check_gate,
};

use common::mock_invocation;

fn make_eval_set_result(passed: usize, failed: usize, cost: f64) -> EvalSetResult {
    let total = passed + failed;
    let mut case_results = Vec::new();

    for i in 0..total {
        let is_pass = i < passed;
        case_results.push(EvalCaseResult {
            case_id: format!("case_{i}"),
            #[allow(clippy::cast_precision_loss)]
            invocation: mock_invocation(&[], Some("response"), cost / total as f64, 100),
            metric_results: Vec::new(),
            verdict: if is_pass {
                Verdict::Pass
            } else {
                Verdict::Fail
            },
        });
    }

    EvalSetResult {
        eval_set_id: "test-set".to_string(),
        case_results,
        summary: EvalSummary {
            total_cases: total,
            passed,
            failed,
            total_cost: Cost {
                total: cost,
                ..Default::default()
            },
            total_usage: Usage::default(),
            total_duration: Duration::from_millis(100),
        },
        timestamp: 0,
    }
}

#[test]
fn gate_integration_pass() {
    let result = make_eval_set_result(9, 1, 0.5);
    let config = GateConfig::new().with_min_pass_rate(0.8).with_max_cost(1.0);
    let gate = check_gate(&result, &config);
    assert!(gate.passed);
    assert_eq!(gate.exit_code, 0);
    assert!(gate.summary.starts_with("GATE PASSED"));
}

#[test]
fn gate_integration_fail() {
    let result = make_eval_set_result(5, 5, 0.5);
    let config = GateConfig::new().with_min_pass_rate(0.9);
    let gate = check_gate(&result, &config);
    assert!(!gate.passed);
    assert_eq!(gate.exit_code, 1);
    assert!(gate.summary.starts_with("GATE FAILED"));
}
