//! Integration tests for the agent/trajectory-family evaluators (T068).
//!
//! All nine judge-backed evaluators drive through `MockJudge` to cover:
//! * happy paths (score + reason non-empty)
//! * FR-020 `None`-on-missing-criterion
//! * `prompt_version` recording in `details`

#![cfg(all(feature = "judge-core", feature = "evaluator-agent"))]

use std::sync::Arc;

use swink_agent_eval::{
    AgentToneEvaluator, Assertion, AssertionKind, Evaluator, ExpectedToolCall,
    InteractionExpectation, InteractionsEvaluator, JudgeClient, JudgeEvaluatorConfig,
    JudgeRegistry, JudgeVerdict, KnowledgeRetentionEvaluator, LanguageDetectionEvaluator,
    MockJudge, PerceivedErrorEvaluator, TaskCompletionEvaluator, TrajectoryAccuracyEvaluator,
    TrajectoryAccuracyWithRefEvaluator, UserSatisfactionEvaluator,
};

mod common;

use common::{mock_invocation, mock_invocation_with_response};

// ─── Helpers ─────────────────────────────────────────────────────────────────

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

fn base_case() -> swink_agent_eval::EvalCase {
    common::case_with_trajectory(vec![])
}

// ─── TrajectoryAccuracyEvaluator ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trajectory_accuracy_happy_path() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        0.9,
        "reasonable tool path",
    )]));
    let evaluator = TrajectoryAccuracyEvaluator::new(config(Arc::clone(&judge)));
    let case = base_case();
    let invocation = mock_invocation_with_response(&[], "The answer is 42.");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("trajectory_accuracy emits a result when criterion met");

    assert_eq!(result.evaluator_name, "trajectory_accuracy");
    assert!((result.score.value - 0.9).abs() < f64::EPSILON);
    let details = result.details.expect("details populated");
    assert!(!details.is_empty(), "details should be non-empty");
    assert!(details.contains("reasonable tool path"));
}

#[test]
fn trajectory_accuracy_returns_none_without_final_response() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = TrajectoryAccuracyEvaluator::new(config(judge));
    let case = base_case();
    let mut invocation = mock_invocation(&[], None, 0.0, 0);
    invocation.final_response = None;

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

#[test]
fn trajectory_accuracy_returns_none_without_user_prompt() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = TrajectoryAccuracyEvaluator::new(config(judge));
    let mut case = base_case();
    case.user_messages.clear();
    let invocation = mock_invocation_with_response(&[], "some response");

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

// ─── TrajectoryAccuracyWithRefEvaluator ──────────────────────────────────────

#[test]
fn trajectory_accuracy_with_ref_requires_trajectory() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = TrajectoryAccuracyWithRefEvaluator::new(config(judge));
    // case_with_trajectory sets expected_trajectory to Some(vec![]) — clear it.
    let mut case = base_case();
    case.expected_trajectory = None;
    let invocation = mock_invocation_with_response(&[], "some response");

    assert!(
        evaluator.evaluate(&case, &invocation).is_none(),
        "should return None when expected_trajectory is absent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trajectory_accuracy_with_ref_happy_path() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        1.0,
        "trajectory matches reference",
    )]));
    let evaluator = TrajectoryAccuracyWithRefEvaluator::new(config(Arc::clone(&judge)));
    let mut case = base_case();
    case.expected_trajectory = Some(vec![ExpectedToolCall {
        tool_name: "search".to_string(),
        arguments: None,
    }]);
    let invocation = mock_invocation_with_response(&["search"], "Found the answer.");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("trajectory_accuracy_with_ref emits a result when trajectory present");

    assert_eq!(result.evaluator_name, "trajectory_accuracy_with_ref");
    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
    let details = result.details.expect("details populated");
    assert!(details.contains("trajectory matches reference"));
}

// ─── TaskCompletionEvaluator ──────────────────────────────────────────────────

#[test]
fn task_completion_requires_assertion() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = TaskCompletionEvaluator::new(config(judge));
    let mut case = base_case();
    case.expected_assertion = None;
    let invocation = mock_invocation_with_response(&[], "done");

    assert!(
        evaluator.evaluate(&case, &invocation).is_none(),
        "should return None when expected_assertion is absent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_completion_happy_path() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        1.0,
        "task completed",
    )]));
    let evaluator = TaskCompletionEvaluator::new(config(Arc::clone(&judge)));
    let mut case = base_case();
    case.expected_assertion = Some(Assertion {
        description: "The agent should have retrieved the document.".to_string(),
        kind: AssertionKind::GoalCompleted,
    });
    let invocation = mock_invocation_with_response(&[], "I retrieved the document successfully.");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("task_completion emits a result when assertion present");

    assert_eq!(result.evaluator_name, "task_completion");
    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
    let details = result.details.expect("details populated");
    assert!(details.contains("task completed"));
}

// ─── UserSatisfactionEvaluator ───────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_satisfaction_happy_path() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        0.85,
        "user appears satisfied",
    )]));
    let evaluator = UserSatisfactionEvaluator::new(config(Arc::clone(&judge)));
    let case = base_case();
    let invocation = mock_invocation_with_response(&[], "Here is the information you requested.");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("user_satisfaction emits a result");

    assert_eq!(result.evaluator_name, "user_satisfaction");
    assert!((result.score.value - 0.85).abs() < f64::EPSILON);
    let details = result.details.expect("details populated");
    assert!(details.contains("user appears satisfied"));
}

#[test]
fn user_satisfaction_returns_none_without_user_prompt() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = UserSatisfactionEvaluator::new(config(judge));
    let mut case = base_case();
    case.user_messages.clear();
    let invocation = mock_invocation_with_response(&[], "response text");

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

// ─── AgentToneEvaluator ───────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_tone_happy_path() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        1.0,
        "professional and respectful tone",
    )]));
    let evaluator = AgentToneEvaluator::new(config(Arc::clone(&judge)));
    // No user prompt required — tone is scored on the response alone.
    let mut case = base_case();
    case.user_messages.clear();
    let invocation = mock_invocation_with_response(&[], "I'm happy to help you with that.");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("agent_tone emits a result (no user prompt required)");

    assert_eq!(result.evaluator_name, "agent_tone");
    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
    let details = result.details.expect("details populated");
    assert!(details.contains("professional and respectful tone"));
}

#[test]
fn agent_tone_returns_none_without_final_response() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = AgentToneEvaluator::new(config(judge));
    let case = base_case();
    let mut invocation = mock_invocation(&[], None, 0.0, 0);
    invocation.final_response = None;

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

// ─── KnowledgeRetentionEvaluator ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn knowledge_retention_happy_path() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        0.8,
        "prior context referenced correctly",
    )]));
    let evaluator = KnowledgeRetentionEvaluator::new(config(Arc::clone(&judge)));
    let case = base_case();
    let invocation =
        mock_invocation_with_response(&[], "As I mentioned earlier, the answer is 42.");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("knowledge_retention emits a result");

    assert_eq!(result.evaluator_name, "knowledge_retention");
    assert!((result.score.value - 0.8).abs() < f64::EPSILON);
}

// ─── LanguageDetectionEvaluator ──────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn language_detection_happy_path() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        1.0,
        "response language: en",
    )]));
    let evaluator = LanguageDetectionEvaluator::new(config(Arc::clone(&judge)));
    let case = base_case();
    let invocation = mock_invocation_with_response(&[], "Here is your answer in English.");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("language_detection emits a result");

    assert_eq!(result.evaluator_name, "language_detection");
    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
}

// ─── PerceivedErrorEvaluator ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn perceived_error_happy_path() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        1.0,
        "no perceived errors",
    )]));
    let evaluator = PerceivedErrorEvaluator::new(config(Arc::clone(&judge)));
    // No user prompt required — error signals are scored on the response alone.
    let mut case = base_case();
    case.user_messages.clear();
    let invocation = mock_invocation_with_response(&[], "Everything completed successfully.");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("perceived_error emits a result (no user prompt required)");

    assert_eq!(result.evaluator_name, "perceived_error");
    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
}

#[test]
fn perceived_error_returns_none_without_final_response() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = PerceivedErrorEvaluator::new(config(judge));
    let case = base_case();
    let mut invocation = mock_invocation(&[], None, 0.0, 0);
    invocation.final_response = None;

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

// ─── InteractionsEvaluator ───────────────────────────────────────────────────

#[test]
fn interactions_requires_expected_interactions() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = InteractionsEvaluator::new(config(judge));
    let mut case = base_case();
    case.expected_interactions = None;
    let invocation = mock_invocation_with_response(&[], "hand-off complete");

    assert!(
        evaluator.evaluate(&case, &invocation).is_none(),
        "should return None when expected_interactions is absent"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interactions_happy_path() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(
        0.95,
        "topology respected",
    )]));
    let evaluator = InteractionsEvaluator::new(config(Arc::clone(&judge)));
    let mut case = base_case();
    case.expected_interactions = Some(vec![InteractionExpectation {
        from: "orchestrator".to_string(),
        to: "search_agent".to_string(),
        description: "Query delegation".to_string(),
    }]);
    let invocation = mock_invocation_with_response(&[], "The search agent found results.");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("interactions emits a result when expected_interactions present");

    assert_eq!(result.evaluator_name, "interactions");
    assert!((result.score.value - 0.95).abs() < f64::EPSILON);
    let details = result.details.expect("details populated");
    assert!(details.contains("topology respected"));
}

// ─── Prompt version recorded (FR-011) ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_version_recorded() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(0.7, "ok")]));
    let evaluator = TrajectoryAccuracyEvaluator::new(config(Arc::clone(&judge)));
    let case = base_case();
    let invocation = mock_invocation_with_response(&[], "trajectory was reasonable");

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("result produced");

    let details = result.details.expect("details present");
    assert!(
        details.contains("trajectory_accuracy_v0"),
        "expected prompt version 'trajectory_accuracy_v0' in details, got: {details}"
    );
}
