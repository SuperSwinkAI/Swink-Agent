//! Integration tests for `SemanticToolSelectionEvaluator` (Spec 023 Phase 9 / US5).
//!
//! Covers AS-5.1 through AS-5.5, the outer `tokio::time::timeout` guarantee
//! (FR-010 / FR-014), empty-trajectory opt-out, and registry wiring.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use swink_agent_eval::{
    EvalCase, Evaluator, EvaluatorRegistry, JudgeClient, JudgeError, JudgeVerdict, MockJudge,
    SemanticToolSelectionEvaluator, SlowMockJudge, Verdict,
};

use common::{mock_invocation, mock_invocation_with_response};

/// Build an `EvalCase` with `semantic_tool_selection = true`, no expected
/// trajectory (the semantic evaluator does not consume one).
fn case_with_semantic_flag(id: &str, user_goal: &str, enabled: bool) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        name: id.to_string(),
        description: None,
        system_prompt: "be helpful".to_string(),
        user_messages: vec![user_goal.to_string()],
        expected_trajectory: None,
        expected_response: None,
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
        semantic_tool_selection: enabled,
        state_capture: None,
    }
}

// ── AS-5.1 ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn semantically_equivalent_tool_accepted() {
    let verdict = JudgeVerdict {
        score: 1.0,
        pass: true,
        reason: Some("fetch_document is equivalent to read_file".into()),
        label: Some("equivalent".into()),
    };
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict]));
    let evaluator = SemanticToolSelectionEvaluator::new(Arc::clone(&judge));

    let case = case_with_semantic_flag("as-5-1", "read the config file", true);
    let invocation = mock_invocation(&["fetch_document"], Some("ok"), 0.0, 0);

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("semantic evaluator must return a metric when flag is enabled");

    assert_eq!(result.evaluator_name, "semantic_tool_selection");
    assert_eq!(result.score.verdict(), Verdict::Pass);
    let details = result.details.unwrap();
    assert!(
        details.contains("equivalent"),
        "expected judge reason in details, got: {details}"
    );
}

// ── AS-5.2 ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_judge_configured_evaluator_absent_from_defaults() {
    // with_defaults() explicitly does NOT register the semantic evaluator.
    let registry = EvaluatorRegistry::with_defaults();
    let mut case = case_with_semantic_flag("as-5-2", "do the thing", true);
    // Give the case *something* to evaluate so other default evaluators engage
    // (response matcher here).
    case.expected_response = Some(swink_agent_eval::ResponseCriteria::Contains {
        substring: "ok".into(),
    });
    let invocation = mock_invocation_with_response(&["read_file"], "ok");

    let results = registry.evaluate(&case, &invocation);
    assert!(
        results
            .iter()
            .all(|r| r.evaluator_name != "semantic_tool_selection"),
        "semantic_tool_selection must not appear when no judge is configured: {results:?}"
    );
}

// ── AS-5.3 ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn semantic_flag_false_returns_none() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = SemanticToolSelectionEvaluator::new(judge);
    let case = case_with_semantic_flag("as-5-3", "do the thing", false);
    let invocation = mock_invocation(&["read_file"], None, 0.0, 0);
    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

// ── AS-5.4 ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_judge_response_yields_score_fail_with_parse_error() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_err(
        JudgeError::MalformedResponse("missing field `score`".into()),
    ));
    let evaluator = SemanticToolSelectionEvaluator::new(judge);
    let case = case_with_semantic_flag("as-5-4", "do the thing", true);
    let invocation = mock_invocation(&["read_file"], None, 0.0, 0);

    let result = evaluator.evaluate(&case, &invocation).unwrap();
    assert_eq!(result.score.verdict(), Verdict::Fail);
    let details = result.details.unwrap();
    assert!(details.contains("MalformedResponse"), "details: {details}");
    assert!(details.contains("missing field"), "details: {details}");
}

// ── AS-5.5 ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transport_error_fails_case_but_registry_continues() {
    use swink_agent_eval::ResponseCriteria;

    // Case 1: judge returns transport error. Case 2: judge returns a pass.
    // We use a single MockJudge with a scripted mixed sequence so a single
    // registry+judge pair can run both cases.
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::mixed_sequence(vec![
        Err(JudgeError::Transport("connection refused".into())),
        Ok(JudgeVerdict {
            score: 1.0,
            pass: true,
            reason: Some("ok".into()),
            label: None,
        }),
    ]));
    let registry = EvaluatorRegistry::with_defaults_and_judge(Arc::clone(&judge));

    let mut case1 = case_with_semantic_flag("case-1", "goal 1", true);
    case1.expected_response = Some(ResponseCriteria::Contains {
        substring: "ok".into(),
    });
    let mut case2 = case_with_semantic_flag("case-2", "goal 2", true);
    case2.expected_response = Some(ResponseCriteria::Contains {
        substring: "ok".into(),
    });

    let inv1 = mock_invocation_with_response(&["read_file"], "ok");
    let inv2 = mock_invocation_with_response(&["read_file"], "ok");

    let results1 = registry.evaluate(&case1, &inv1);
    let semantic1 = results1
        .iter()
        .find(|r| r.evaluator_name == "semantic_tool_selection")
        .expect("case 1 semantic result");
    assert_eq!(semantic1.score.verdict(), Verdict::Fail);
    assert!(
        semantic1
            .details
            .as_deref()
            .unwrap_or_default()
            .contains("Transport"),
        "expected Transport in details: {:?}",
        semantic1.details
    );

    // Registry continues — case 2 still runs and returns a Pass for the
    // semantic evaluator using the second queued verdict.
    let results2 = registry.evaluate(&case2, &inv2);
    let semantic2 = results2
        .iter()
        .find(|r| r.evaluator_name == "semantic_tool_selection")
        .expect("case 2 semantic result");
    assert_eq!(semantic2.score.verdict(), Verdict::Pass);

    // And the non-semantic evaluators still fired for both cases.
    assert!(
        results1.iter().any(|r| r.evaluator_name == "response"),
        "case 1 response should still run"
    );
    assert!(
        results2.iter().any(|r| r.evaluator_name == "response"),
        "case 2 response should still run"
    );
}

// ── T054a: outer tokio timeout (FR-010) ────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outer_timeout_maps_to_fail_and_does_not_hang() {
    // SlowMockJudge sleeps 10s; evaluator's outer timeout is 50ms.
    let judge: Arc<dyn JudgeClient> = Arc::new(SlowMockJudge::new(Duration::from_secs(10)));
    let evaluator =
        SemanticToolSelectionEvaluator::new(judge).with_timeout(Duration::from_millis(50));

    let case = case_with_semantic_flag("as-5-timeout", "do the thing", true);
    let invocation = mock_invocation(&["read_file"], None, 0.0, 0);

    let start = Instant::now();
    let result = evaluator.evaluate(&case, &invocation).unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "outer timeout did not fire promptly — elapsed: {elapsed:?}"
    );
    assert_eq!(result.score.verdict(), Verdict::Fail);
    let details = result.details.unwrap();
    assert!(
        details.contains("exceeded"),
        "expected timeout context in details, got: {details}"
    );
}

// ── T055: empty trajectory ─────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_trajectory_returns_none() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = SemanticToolSelectionEvaluator::new(judge);
    let case = case_with_semantic_flag("empty-traj", "do the thing", true);
    // mock_invocation with empty names produces zero tool calls.
    let invocation = mock_invocation(&[], None, 0.0, 0);
    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

// ── Runtime-agnostic invocation ────────────────────────────────────────────
//
// Regression for PR #764 review feedback: `Evaluator::evaluate` is sync, and
// an earlier version called `tokio::runtime::Handle::current()` which
// panics outside an active Tokio runtime. The evaluator must now build its
// own ephemeral runtime when called from a plain sync context.

#[test]
fn evaluates_from_plain_sync_context_without_panic() {
    let verdict = JudgeVerdict {
        score: 1.0,
        pass: true,
        reason: Some("ok".into()),
        label: None,
    };
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict]));
    let evaluator = SemanticToolSelectionEvaluator::new(judge);
    let case = case_with_semantic_flag("sync-ctx", "do the thing", true);
    let invocation = mock_invocation(&["fetch_document"], Some("ok"), 0.0, 0);

    // No #[tokio::test] attribute — this runs on the raw test thread with
    // no ambient runtime.
    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("must produce a metric without an ambient runtime");
    assert_eq!(result.score.verdict(), Verdict::Pass);
}
