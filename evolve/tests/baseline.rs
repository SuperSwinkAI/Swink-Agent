//! US1: baseline evaluation tests.

use std::sync::Arc;

use swink_agent::{Agent, AgentOptions, ModelSpec, testing::SimpleMockStreamFn};
use swink_agent_eval::{AgentFactory, EvalCase, EvalError, EvalSet, ResponseCriteria, Score};
use tokio_util::sync::CancellationToken;

use swink_agent_evolve::runner::EvolutionRunner;
use swink_agent_evolve::{OptimizationConfig, OptimizationTarget};

struct EchoFactory;

impl AgentFactory for EchoFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let stream_fn = Arc::new(SimpleMockStreamFn::new(vec!["ok".to_string()]));
        let model = ModelSpec::new("test", "test-model");
        let options = AgentOptions::new_simple(&case.system_prompt, model, stream_fn);
        Ok((Agent::new(options), CancellationToken::new()))
    }
}

fn case_with_custom_score(id: &str, score_value: f64) -> EvalCase {
    EvalCase::new(
        id,
        id,
        "You are a test assistant.",
        vec!["hello".to_string()],
    )
    .with_expected_response(ResponseCriteria::Custom(Arc::new(move |_: &str| {
        Score::new(score_value, 0.5)
    })))
}

fn make_runner(cases: Vec<EvalCase>) -> EvolutionRunner {
    let target = OptimizationTarget::new("You are helpful.", vec![]);
    let eval_set = EvalSet::new("test", "Test", cases);
    let config = OptimizationConfig::new(eval_set, "/tmp/evolve-baseline-test");
    EvolutionRunner::new(target, config, Arc::new(EchoFactory), None)
}

#[tokio::test]
async fn baseline_returns_per_case_scores() {
    let runner = make_runner(vec![
        case_with_custom_score("c1", 0.8),
        case_with_custom_score("c2", 0.6),
        case_with_custom_score("c3", 1.0),
    ]);
    let baseline = runner.baseline().await.unwrap();
    assert_eq!(baseline.results.len(), 3);
    // Every case has at least one metric result from ResponseMatcher
    for result in &baseline.results {
        assert!(!result.metric_results.is_empty());
    }
}

#[tokio::test]
async fn baseline_aggregate_is_arithmetic_mean() {
    let runner = make_runner(vec![
        case_with_custom_score("c1", 0.8),
        case_with_custom_score("c2", 0.6),
        case_with_custom_score("c3", 1.0),
    ]);
    let baseline = runner.baseline().await.unwrap();
    let expected = (0.8_f64 + 0.6 + 1.0) / 3.0;
    assert!(
        (baseline.aggregate_score - expected).abs() < 1e-10,
        "expected aggregate {expected:.6}, got {:.6}",
        baseline.aggregate_score
    );
}

#[tokio::test]
async fn baseline_records_failures_with_details() {
    // Mock returns "ok" but case expects "match me" — ResponseMatcher produces failure details.
    let failing_case = EvalCase::new(
        "failing",
        "Failing",
        "You are a test assistant.",
        vec!["hello".to_string()],
    )
    .with_expected_response(ResponseCriteria::Exact {
        expected: "this will not match".to_string(),
    });
    let runner = make_runner(vec![failing_case]);
    let baseline = runner.baseline().await.unwrap();
    assert_eq!(baseline.results.len(), 1);
    let case_result = &baseline.results[0];
    assert!(
        !case_result.metric_results.is_empty(),
        "failing case should produce metric results"
    );
    let metric = &case_result.metric_results[0];
    assert!(
        metric.details.is_some(),
        "failure details should be propagated from evaluator"
    );
    assert_eq!(metric.score.value, 0.0, "response mismatch should score 0");
}
