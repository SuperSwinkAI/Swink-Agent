//! US4+US5: candidate evaluation and acceptance gate tests.

use std::collections::HashMap;
use std::time::Duration;

use swink_agent::{Cost, ModelSpec, StopReason, Usage};
use swink_agent_eval::{EvalCaseResult, EvalMetricResult, Invocation, Score, Verdict};

use swink_agent_evolve::{
    AcceptanceGate, AcceptanceVerdict, BaselineSnapshot, Candidate, CandidateResult,
    OptimizationTarget, TargetComponent,
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

fn build_case_result(case_id: &str, score: f64, verdict: Verdict) -> EvalCaseResult {
    EvalCaseResult {
        case_id: case_id.to_string(),
        invocation: make_invocation(),
        metric_results: vec![EvalMetricResult {
            evaluator_name: "response".to_string(),
            score: Score {
                value: score,
                threshold: 0.5,
            },
            details: None,
        }],
        verdict,
    }
}

fn build_baseline(cases: Vec<(&str, f64, Verdict)>, aggregate: f64) -> BaselineSnapshot {
    let results = cases
        .into_iter()
        .map(|(id, s, v)| build_case_result(id, s, v))
        .collect();
    BaselineSnapshot {
        target: OptimizationTarget::new("sys", vec![]),
        results,
        aggregate_score: aggregate,
        cost: Cost::default(),
    }
}

fn make_candidate(component: TargetComponent, mutated: &str) -> Candidate {
    Candidate::new(
        component,
        "original".to_string(),
        mutated.to_string(),
        "test".to_string(),
    )
}

fn build_candidate_result(
    candidate: Candidate,
    cases: Vec<(&str, f64, Verdict)>,
    aggregate: f64,
) -> CandidateResult {
    let results = cases
        .into_iter()
        .map(|(id, s, v)| build_case_result(id, s, v))
        .collect();
    CandidateResult {
        candidate,
        results,
        aggregate_score: aggregate,
        cost: Cost::default(),
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[test]
fn candidate_above_threshold_accepted() {
    // Baseline aggregate = 0.6, candidate = 0.65 (improvement = 0.05 ≥ threshold 0.01)
    // No P1 regressions.
    let baseline = build_baseline(
        vec![("c1", 0.9, Verdict::Pass), ("c2", 0.3, Verdict::Fail)],
        0.6,
    );
    let cand = make_candidate(TargetComponent::FullPrompt, "v2");
    let cr = build_candidate_result(
        cand,
        vec![("c1", 0.9, Verdict::Pass), ("c2", 0.8, Verdict::Pass)],
        0.65,
    );
    let gate = AcceptanceGate::new(0.01);
    let result = gate.evaluate(&baseline, &[cr]);
    assert_eq!(
        result.applied.len(),
        1,
        "candidate should be accepted (applied)"
    );
    assert!(result.accepted_not_applied.is_empty());
    assert!(result.rejected.is_empty());
}

#[test]
fn candidate_below_threshold_rejected() {
    // improvement = 0.005 < threshold 0.01 → BelowThreshold
    let baseline = build_baseline(vec![("c1", 0.9, Verdict::Pass)], 0.6);
    let cand = make_candidate(TargetComponent::FullPrompt, "v2");
    let cr = build_candidate_result(cand, vec![("c1", 0.9, Verdict::Pass)], 0.605);
    let gate = AcceptanceGate::new(0.01);
    let result = gate.evaluate(&baseline, &[cr]);
    assert!(result.applied.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(
        matches!(
            result.rejected[0].2,
            AcceptanceVerdict::BelowThreshold { .. }
        ),
        "expected BelowThreshold, got {:?}",
        result.rejected[0].2
    );
}

#[test]
fn p1_regression_rejected() {
    // Baseline: c1 passes, c2 fails, c3 passes.
    // Candidate: c1 regresses (Pass→Fail), c2+c3 improve → aggregate goes up.
    // c1 has no metadata → default P1 → P1Regression should block acceptance.
    let baseline = build_baseline(
        vec![
            ("c1", 0.9, Verdict::Pass),
            ("c2", 0.3, Verdict::Fail),
            ("c3", 0.5, Verdict::Pass),
        ],
        0.567,
    );
    let cand = make_candidate(TargetComponent::FullPrompt, "v2");
    let cr = build_candidate_result(
        cand,
        vec![
            ("c1", 0.1, Verdict::Fail),
            ("c2", 1.0, Verdict::Pass),
            ("c3", 1.0, Verdict::Pass),
        ],
        0.7, // improvement = 0.133 ≥ 0.01, but c1 regresses
    );
    let gate = AcceptanceGate::new(0.01);
    let result = gate.evaluate(&baseline, &[cr]);
    assert!(
        result.applied.is_empty(),
        "P1 regression should block acceptance"
    );
    assert_eq!(result.rejected.len(), 1);
    match &result.rejected[0].2 {
        AcceptanceVerdict::P1Regression { case_id } => assert_eq!(case_id, "c1"),
        other => panic!("expected P1Regression, got {:?}", other),
    }
}

#[test]
fn top_ranked_per_component() {
    // Two accepted candidates targeting the same component.
    // Higher improvement (v2) → Accepted (applied); lower (v3) → AcceptedNotApplied.
    let baseline = build_baseline(vec![("c1", 0.5, Verdict::Pass)], 0.5);
    let ca = make_candidate(TargetComponent::FullPrompt, "v2");
    let cb = make_candidate(TargetComponent::FullPrompt, "v3");
    let ca_result = build_candidate_result(ca, vec![("c1", 0.9, Verdict::Pass)], 0.55); // +0.05
    let cb_result = build_candidate_result(cb, vec![("c1", 0.8, Verdict::Pass)], 0.53); // +0.03
    let gate = AcceptanceGate::new(0.01);
    let result = gate.evaluate(&baseline, &[ca_result, cb_result]);
    assert_eq!(result.applied.len(), 1);
    assert_eq!(result.accepted_not_applied.len(), 1);
    assert!(result.rejected.is_empty());
    assert_eq!(
        result.applied[0].0.mutated_value, "v2",
        "highest-improvement candidate applied first"
    );
    assert_eq!(result.accepted_not_applied[0].0.mutated_value, "v3");
}

#[test]
fn custom_threshold_enforced() {
    // Threshold = 0.10; candidate improves by 0.08 → rejected.
    let baseline = build_baseline(vec![("c1", 0.5, Verdict::Pass)], 0.5);
    let cand = make_candidate(TargetComponent::FullPrompt, "v2");
    let cr = build_candidate_result(cand, vec![("c1", 0.8, Verdict::Pass)], 0.58); // +0.08 < 0.10
    let gate = AcceptanceGate::new(0.10);
    let result = gate.evaluate(&baseline, &[cr]);
    assert!(result.applied.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(
        matches!(
            result.rejected[0].2,
            AcceptanceVerdict::BelowThreshold { .. }
        ),
        "expected BelowThreshold for improvement below custom threshold"
    );
}

#[test]
fn p2_case_regression_allowed() {
    // c1 is explicitly P2; c2 and c3 improve; aggregate goes up.
    // c1 regresses (Pass→Fail) but since it's P2, gate should still accept.
    let baseline = build_baseline(
        vec![
            ("c1", 0.9, Verdict::Pass),
            ("c2", 0.3, Verdict::Fail),
            ("c3", 0.5, Verdict::Pass),
        ],
        0.567,
    );
    let cand = make_candidate(TargetComponent::FullPrompt, "v2");
    let cr = build_candidate_result(
        cand,
        vec![
            ("c1", 0.1, Verdict::Fail),
            ("c2", 1.0, Verdict::Pass),
            ("c3", 1.0, Verdict::Pass),
        ],
        0.7, // improvement = 0.133 ≥ 0.01
    );
    let mut meta = HashMap::new();
    meta.insert("c1".to_string(), serde_json::json!({"priority": "P2"}));
    let gate = AcceptanceGate::new(0.01).with_case_metadata(meta);
    let result = gate.evaluate(&baseline, &[cr]);
    assert_eq!(
        result.applied.len(),
        1,
        "P2 regression should not block acceptance; got rejected: {:?}",
        result
            .rejected
            .iter()
            .map(|(_, _, v)| v)
            .collect::<Vec<_>>()
    );
    assert!(result.rejected.is_empty());
}
