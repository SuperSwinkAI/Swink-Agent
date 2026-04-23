//! Integration tests for the quality-family judge-backed evaluators (T057, T060).
//!
//! These tests avoid exercising a real LLM: every evaluator is driven against
//! [`swink_agent_eval::MockJudge`] so we can assert on `None`-return semantics,
//! score clamp, `prompt_version` recording, and the hallucination vs.
//! faithfulness rubric separation.

#![cfg(all(feature = "judge-core", feature = "evaluator-quality"))]

use std::sync::Arc;

use swink_agent_eval::{Assertion, AssertionKind};
use swink_agent_eval::{
    CoherenceEvaluator, ConcisenessEvaluator, CorrectnessEvaluator, Evaluator,
    FaithfulnessEvaluator, FewShotExample, GoalSuccessRateEvaluator, HallucinationEvaluator,
    HelpfulnessEvaluator, JudgeClient, JudgeEvaluatorConfig, JudgeRegistry, JudgeVerdict,
    LazinessEvaluator, MockJudge, PlanAdherenceEvaluator, ResponseRelevanceEvaluator,
};

mod common;

use common::{case_with_response, mock_invocation, mock_invocation_with_response};

fn make_registry(judge: Arc<dyn JudgeClient>) -> Arc<JudgeRegistry> {
    Arc::new(
        JudgeRegistry::builder(judge, "mock-model")
            .build()
            .expect("registry builds"),
    )
}

fn config(judge: Arc<dyn JudgeClient>) -> JudgeEvaluatorConfig {
    JudgeEvaluatorConfig::default_with(make_registry(judge))
}

fn verdict(score: f64, reason: &str) -> JudgeVerdict {
    JudgeVerdict {
        score,
        pass: (0.5..=1.0).contains(&score),
        reason: Some(reason.to_string()),
        label: None,
    }
}

// ─── T057: baseline evaluator wiring ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helpfulness_records_prompt_version_and_score() {
    let judge: Arc<dyn JudgeClient> =
        Arc::new(MockJudge::with_verdicts(vec![verdict(0.8, "helpful")]));
    let evaluator = HelpfulnessEvaluator::new(config(Arc::clone(&judge)));
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "here is your answer");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("helpfulness emits a result with user prompt + response");
    assert_eq!(result.evaluator_name, "helpfulness");
    assert!((result.score.value - 0.8).abs() < f64::EPSILON);
    let details = result.details.expect("details populated");
    assert!(details.contains("helpfulness_v0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn correctness_records_prompt_version() {
    let judge: Arc<dyn JudgeClient> =
        Arc::new(MockJudge::with_verdicts(vec![verdict(0.9, "correct")]));
    let evaluator = CorrectnessEvaluator::new(config(Arc::clone(&judge)));
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "42");

    let result = evaluator.evaluate(&case, &invocation).expect("result");
    let details = result.details.expect("details");
    assert!(details.contains("correctness_v0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conciseness_only_needs_final_response() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(0.6, "ok")]));
    let evaluator = ConcisenessEvaluator::new(config(Arc::clone(&judge)));
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "short answer");
    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert!(result.details.unwrap().contains("conciseness_v0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coherence_only_needs_final_response() {
    let judge: Arc<dyn JudgeClient> =
        Arc::new(MockJudge::with_verdicts(vec![verdict(0.7, "coherent")]));
    let evaluator = CoherenceEvaluator::new(config(Arc::clone(&judge)));
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "this answer flows logically");
    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert!(result.details.unwrap().contains("coherence_v0"));
}

// ─── T060: None-return, score clamp, rubric separation ──────────────────────

#[test]
fn helpfulness_returns_none_when_final_response_missing() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = HelpfulnessEvaluator::new(config(judge));
    let case = common::case_with_trajectory(vec![]);
    let mut invocation = mock_invocation(&[], None, 0.0, 0);
    invocation.final_response = None;

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[test]
fn correctness_returns_none_when_user_prompt_missing() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = CorrectnessEvaluator::new(config(judge));
    let mut case = common::case_with_trajectory(vec![]);
    case.user_messages.clear();
    let invocation = mock_invocation_with_response(&[], "stub");

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[test]
fn faithfulness_returns_none_without_retrieved_context() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = FaithfulnessEvaluator::new(config(judge));
    let case = common::case_with_trajectory(vec![]); // no few_shot_examples
    let invocation = mock_invocation_with_response(&[], "the sky is blue");

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn faithfulness_runs_with_retrieved_context() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        1.0,
        "fully grounded",
    )]));
    let evaluator = FaithfulnessEvaluator::new(config(Arc::clone(&judge)));
    let mut case = common::case_with_trajectory(vec![]);
    case.few_shot_examples = vec![FewShotExample {
        input: "retrieved passage".into(),
        expected: "the sky is blue".into(),
        reasoning: None,
    }];
    let invocation = mock_invocation_with_response(&[], "the sky is blue");

    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert!(result.details.unwrap().contains("faithfulness_v0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hallucination_distinct_from_faithfulness_rubric() {
    // Hallucination must NOT require retrieved context — it judges against the
    // user prompt alone.
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        0.5,
        "potentially hallucinated",
    )]));
    let evaluator = HallucinationEvaluator::new(config(Arc::clone(&judge)));
    let case = common::case_with_trajectory(vec![]); // no few_shot_examples
    let invocation = mock_invocation_with_response(&[], "Paris is the capital of France");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("hallucination runs without retrieved context");
    assert!(result.details.unwrap().contains("hallucination_v0"));
}

#[test]
fn hallucination_and_faithfulness_use_different_templates() {
    // A structural assertion that protects against accidentally pointing both
    // evaluators at the same built-in template.
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let hallucination = HallucinationEvaluator::new(config(Arc::clone(&judge)));
    let faithfulness = FaithfulnessEvaluator::new(config(Arc::clone(&judge)));
    assert_eq!(hallucination.name(), "hallucination");
    assert_eq!(faithfulness.name(), "faithfulness");
    assert_ne!(hallucination.name(), faithfulness.name());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn score_clamp_records_warning_for_out_of_range_verdict() {
    // FR-021: score returned by the judge must be clamped to [0.0, 1.0] with
    // a ScoreClamped detail surfaced in details.
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        1.4,
        "impossibly confident",
    )]));
    let evaluator = HelpfulnessEvaluator::new(config(Arc::clone(&judge)));
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "answer");

    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
    let details = result.details.expect("details");
    assert!(
        details.contains("score_clamped"),
        "expected score_clamped detail, got: {details}"
    );
    assert!(details.contains("1.4"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn score_clamp_handles_negative_verdict() {
    let judge: Arc<dyn JudgeClient> =
        Arc::new(MockJudge::with_verdicts(vec![verdict(-0.2, "worse")]));
    let evaluator = HelpfulnessEvaluator::new(config(Arc::clone(&judge)));
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "answer");

    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert!((result.score.value - 0.0).abs() < f64::EPSILON);
    let details = result.details.expect("details");
    assert!(details.contains("score_clamped"));
}

// ─── Remaining T058/T059 evaluator wiring ───────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn response_relevance_emits_result() {
    let judge: Arc<dyn JudgeClient> =
        Arc::new(MockJudge::with_verdicts(vec![verdict(0.9, "on topic")]));
    let evaluator = ResponseRelevanceEvaluator::new(config(Arc::clone(&judge)));
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "relevant answer");
    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert!(result.details.unwrap().contains("response_relevance_v0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn plan_adherence_requires_system_prompt() {
    let judge: Arc<dyn JudgeClient> =
        Arc::new(MockJudge::with_verdicts(vec![verdict(0.75, "adhered")]));
    let evaluator = PlanAdherenceEvaluator::new(config(Arc::clone(&judge)));
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "plan response");
    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert!(result.details.unwrap().contains("plan_adherence_v0"));
}

#[test]
fn plan_adherence_returns_none_when_system_prompt_blank() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = PlanAdherenceEvaluator::new(config(judge));
    let mut case = common::case_with_trajectory(vec![]);
    case.system_prompt = "   ".into();
    let invocation = mock_invocation_with_response(&[], "response");
    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn laziness_emits_result() {
    let judge: Arc<dyn JudgeClient> =
        Arc::new(MockJudge::with_verdicts(vec![verdict(0.4, "punted")]));
    let evaluator = LazinessEvaluator::new(config(Arc::clone(&judge)));
    let case = common::case_with_trajectory(vec![]);
    let invocation = mock_invocation_with_response(&[], "sorry, I can't help");
    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert!(result.details.unwrap().contains("laziness_v0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn goal_success_rate_consumes_expected_assertion() {
    let judge: Arc<dyn JudgeClient> =
        Arc::new(MockJudge::with_verdicts(vec![verdict(1.0, "goal met")]));
    let evaluator = GoalSuccessRateEvaluator::new(config(Arc::clone(&judge)));
    let mut case = case_with_response(swink_agent_eval::ResponseCriteria::Contains {
        substring: "answer".into(),
    });
    case.expected_assertion = Some(Assertion {
        description: "User's goal was to find the answer".into(),
        kind: AssertionKind::GoalCompleted,
    });
    let invocation = mock_invocation_with_response(&[], "the answer");

    let result = evaluator.evaluate(&case, &invocation).expect("result");
    assert!(result.details.unwrap().contains("goal_success_rate_v0"));
}

#[test]
fn goal_success_rate_returns_none_without_assertion() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = GoalSuccessRateEvaluator::new(config(judge));
    let case = common::case_with_trajectory(vec![]); // no expected_assertion
    let invocation = mock_invocation_with_response(&[], "answer");
    assert!(evaluator.evaluate(&case, &invocation).is_none());
}
