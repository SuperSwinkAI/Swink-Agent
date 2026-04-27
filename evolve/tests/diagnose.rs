//! US2: diagnose weak points tests.

use std::time::Duration;

use swink_agent::{Cost, ModelSpec, StopReason, Usage};
use swink_agent_eval::{EvalCaseResult, EvalMetricResult, Invocation, Score, Verdict};

use swink_agent_evolve::{
    BaselineSnapshot, Diagnoser, OptimizationTarget, TargetComponent,
};

fn make_invocation() -> Invocation {
    Invocation {
        turns: vec![],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::ZERO,
        final_response: None,
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "test-model"),
    }
}

fn failing_metric(
    evaluator_name: &str,
    value: f64,
    threshold: f64,
    details: Option<String>,
) -> EvalMetricResult {
    EvalMetricResult { evaluator_name: evaluator_name.to_string(), score: Score { value, threshold }, details }
}

fn passing_metric(evaluator_name: &str, value: f64, threshold: f64) -> EvalMetricResult {
    EvalMetricResult {
        evaluator_name: evaluator_name.to_string(),
        score: Score { value, threshold },
        details: None,
    }
}

fn build_case_result(case_id: &str, metrics: Vec<EvalMetricResult>) -> EvalCaseResult {
    EvalCaseResult {
        case_id: case_id.to_string(),
        invocation: make_invocation(),
        metric_results: metrics,
        verdict: Verdict::Fail,
    }
}

fn build_baseline(results: Vec<EvalCaseResult>) -> BaselineSnapshot {
    let aggregate = BaselineSnapshot::aggregate_from_results(&results);
    BaselineSnapshot {
        target: OptimizationTarget::new("sys", vec![]),
        results,
        aggregate_score: aggregate,
        cost: Cost::default(),
    }
}

#[test]
fn diagnose_identifies_tool_failure() {
    let cases = vec![
        build_case_result("c1", vec![failing_metric("tool:my_tool", 0.2, 0.5, None)]),
        build_case_result("c2", vec![failing_metric("tool:my_tool", 0.1, 0.5, None)]),
        build_case_result("c3", vec![failing_metric("tool:my_tool", 0.3, 0.5, None)]),
    ];
    let baseline = build_baseline(cases);
    let diagnoser = Diagnoser::new(5);
    let target = OptimizationTarget::new("sys", vec![]);
    let weak_points = diagnoser.diagnose(&baseline, &target);
    assert_eq!(weak_points.len(), 1);
    assert_eq!(
        weak_points[0].component,
        TargetComponent::ToolDescription { tool_name: "my_tool".to_string() }
    );
    assert_eq!(weak_points[0].affected_cases.len(), 3);
}

#[test]
fn diagnose_identifies_prompt_failure() {
    let cases = vec![build_case_result(
        "c1",
        vec![failing_metric("response", 0.0, 0.5, Some("mismatch".into()))],
    )];
    let baseline = build_baseline(cases);
    let diagnoser = Diagnoser::new(5);
    let target = OptimizationTarget::new("sys", vec![]);
    let weak_points = diagnoser.diagnose(&baseline, &target);
    assert_eq!(weak_points.len(), 1);
    assert!(
        matches!(
            weak_points[0].component,
            TargetComponent::FullPrompt | TargetComponent::PromptSection { .. }
        ),
        "expected FullPrompt or PromptSection, got {:?}",
        weak_points[0].component
    );
}

#[test]
fn diagnose_returns_empty_for_passing_baseline() {
    let cases = vec![
        build_case_result("c1", vec![passing_metric("response", 0.95, 0.5)]),
        build_case_result("c2", vec![passing_metric("response", 0.90, 0.5)]),
    ];
    let baseline = build_baseline(cases);
    let diagnoser = Diagnoser::new(5);
    let target = OptimizationTarget::new("sys", vec![]);
    let weak_points = diagnoser.diagnose(&baseline, &target);
    assert!(weak_points.is_empty());
}

#[test]
fn diagnose_ranks_by_severity() {
    // FullPrompt group: 3 cases, each gap = 0.5 - 0.1 = 0.4 → severity = 3 × 0.4 = 1.2
    // ToolDescription group: 1 case, gap = 0.9 - 0.0 = 0.9 → severity = 1 × 0.9 = 0.9
    // Expected order: FullPrompt (1.2) before ToolDescription (0.9)
    let cases = vec![
        build_case_result("c1", vec![failing_metric("response", 0.1, 0.5, None)]),
        build_case_result("c2", vec![failing_metric("response", 0.1, 0.5, None)]),
        build_case_result("c3", vec![failing_metric("response", 0.1, 0.5, None)]),
        build_case_result("c4", vec![failing_metric("tool:other_tool", 0.0, 0.9, None)]),
    ];
    let baseline = build_baseline(cases);
    let diagnoser = Diagnoser::new(5);
    let target = OptimizationTarget::new("sys", vec![]);
    let weak_points = diagnoser.diagnose(&baseline, &target);
    assert_eq!(weak_points.len(), 2);
    assert!(
        matches!(weak_points[0].component, TargetComponent::FullPrompt),
        "highest severity should be FullPrompt, got {:?}",
        weak_points[0].component
    );
    assert!(
        matches!(weak_points[1].component, TargetComponent::ToolDescription { .. }),
        "second should be ToolDescription, got {:?}",
        weak_points[1].component
    );
    assert!(
        weak_points[0].severity > weak_points[1].severity,
        "FullPrompt severity {:.3} should exceed ToolDescription severity {:.3}",
        weak_points[0].severity,
        weak_points[1].severity
    );
}
