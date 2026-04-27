//! US1: baseline evaluation tests.

use std::sync::Arc;

use swink_agent::{Agent, AgentOptions, ModelSpec, testing::SimpleMockStreamFn};
use swink_agent_eval::{
    AgentFactory, EvalCase, EvalError, EvalSet, ResponseCriteria, Score,
};
use tokio_util::sync::CancellationToken;

use swink_agent_evolve::{OptimizationConfig, OptimizationTarget};
use swink_agent_evolve::runner::EvolutionRunner;

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
    EvalCase {
        id: id.to_string(),
        name: id.to_string(),
        description: None,
        system_prompt: "You are a test assistant.".to_string(),
        user_messages: vec!["hello".to_string()],
        expected_response: Some(ResponseCriteria::Custom(Arc::new(move |_: &str| Score {
            value: score_value,
            threshold: 0.5,
        }))),
        expected_trajectory: None,
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

fn make_runner(cases: Vec<EvalCase>) -> EvolutionRunner {
    let target = OptimizationTarget::new("You are helpful.", vec![]);
    let eval_set = EvalSet { id: "test".into(), name: "Test".into(), description: None, cases };
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
    let failing_case = EvalCase {
        id: "failing".to_string(),
        name: "Failing".to_string(),
        description: None,
        system_prompt: "You are a test assistant.".to_string(),
        user_messages: vec!["hello".to_string()],
        expected_response: Some(ResponseCriteria::Exact {
            expected: "this will not match".to_string(),
        }),
        expected_trajectory: None,
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    };
    let runner = make_runner(vec![failing_case]);
    let baseline = runner.baseline().await.unwrap();
    assert_eq!(baseline.results.len(), 1);
    let case_result = &baseline.results[0];
    assert!(
        !case_result.metric_results.is_empty(),
        "failing case should produce metric results"
    );
    let metric = &case_result.metric_results[0];
    assert!(metric.details.is_some(), "failure details should be propagated from evaluator");
    assert_eq!(metric.score.value, 0.0, "response mismatch should score 0");
}
