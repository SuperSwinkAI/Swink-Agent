//! Integration tests for `SemanticToolParameterEvaluator` (Spec 023 Phase 10 / US6).
//!
//! Covers AS-6.1 through AS-6.4, the inner `JudgeError::Timeout` path, the
//! outer `tokio::time::timeout` guarantee (FR-010 / FR-014), and the
//! tool-name filter matrix (full skip → `None`; partial match → only target
//! judged).

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use swink_agent::{AssistantMessage, Cost, ModelSpec, StopReason, Usage};

use swink_agent_eval::{
    EvalCase, Evaluator, Invocation, JudgeClient, JudgeError, JudgeVerdict, MockJudge,
    RecordedToolCall, SemanticToolParameterEvaluator, SlowMockJudge, ToolIntent, TurnRecord,
    Verdict,
};

use common::mock_invocation;

/// Build an `EvalCase` with an `expected_tool_intent`, nothing else set.
fn case_with_intent(id: &str, intent: &str, tool_name: Option<&str>) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        name: id.to_string(),
        description: None,
        system_prompt: "be helpful".to_string(),
        user_messages: vec!["do the thing".to_string()],
        expected_trajectory: None,
        expected_response: None,
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        expected_environment_state: None,
        expected_tool_intent: Some(ToolIntent {
            intent: intent.to_string(),
            tool_name: tool_name.map(String::from),
        }),
        semantic_tool_selection: false,
        state_capture: None,
    }
}

/// Build an `EvalCase` with no intent set.
fn case_without_intent(id: &str) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        name: id.to_string(),
        description: None,
        system_prompt: "be helpful".to_string(),
        user_messages: vec!["do the thing".to_string()],
        expected_trajectory: None,
        expected_response: None,
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        expected_environment_state: None,
        expected_tool_intent: None,
        semantic_tool_selection: false,
        state_capture: None,
    }
}

/// Build an invocation whose only turn contains arbitrary (name, args) pairs.
fn invocation_with(calls: &[(&str, serde_json::Value)]) -> Invocation {
    let tool_calls: Vec<RecordedToolCall> = calls
        .iter()
        .enumerate()
        .map(|(i, (name, args))| RecordedToolCall {
            id: format!("id{i}"),
            name: (*name).to_string(),
            arguments: args.clone(),
        })
        .collect();

    Invocation {
        turns: vec![TurnRecord {
            turn_index: 0,
            assistant_message: AssistantMessage {
                content: vec![],
                provider: "test".into(),
                model_id: "test-model".into(),
                usage: Usage::default(),
                cost: Cost::default(),
                stop_reason: StopReason::Stop,
                error_message: None,
                error_kind: None,
                timestamp: 0,
                cache_hint: None,
            },
            tool_calls,
            tool_results: vec![],
            duration: Duration::from_millis(1),
        }],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(1),
        final_response: None,
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "test-model"),
    }
}

// ── T058 / AS-6.1 ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn intent_satisfied_by_non_literal_arguments() {
    let verdict = JudgeVerdict {
        score: 1.0,
        pass: true,
        reason: Some("arguments satisfy the intent".into()),
        label: Some("equivalent".into()),
    };
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict]));
    let evaluator = SemanticToolParameterEvaluator::new(Arc::clone(&judge));

    let case = case_with_intent("as-6-1", "read config for project-alpha", None);
    let invocation = invocation_with(&[(
        "read_file",
        serde_json::json!({"path": "./project-alpha/config.toml"}),
    )]);

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("evaluator must return a metric when intent is set");

    assert_eq!(result.evaluator_name, "semantic_tool_parameter");
    assert_eq!(result.score.verdict(), Verdict::Pass);
    let details = result.details.unwrap();
    assert!(
        details.contains("satisfy"),
        "expected judge reason in details, got: {details}"
    );
}

// ── T059 / AS-6.2 ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_expected_tool_intent_returns_none() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_pass());
    let evaluator = SemanticToolParameterEvaluator::new(judge);

    let case = case_without_intent("as-6-2");
    let invocation = mock_invocation(&["read_file"], None, 0.0, 0);

    assert!(evaluator.evaluate(&case, &invocation).is_none());
}

// ── T060 / AS-6.3 ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inner_judge_timeout_maps_to_fail() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::always_err(JudgeError::Timeout));
    let evaluator = SemanticToolParameterEvaluator::new(judge);

    let case = case_with_intent("as-6-3", "read the config", None);
    let invocation = mock_invocation(&["read_file"], None, 0.0, 0);

    let start = Instant::now();
    let result = evaluator.evaluate(&case, &invocation).unwrap();
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "inner timeout should not hang — elapsed: {elapsed:?}"
    );
    assert_eq!(result.score.verdict(), Verdict::Fail);
    let details = result.details.unwrap();
    assert!(details.contains("Timeout"), "details: {details}");
}

// ── T060a: outer tokio timeout (FR-010) ────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outer_timeout_maps_to_fail_and_does_not_hang() {
    // SlowMockJudge sleeps 10s; evaluator's outer timeout is 50ms.
    let judge: Arc<dyn JudgeClient> = Arc::new(SlowMockJudge::new(Duration::from_secs(10)));
    let evaluator =
        SemanticToolParameterEvaluator::new(judge).with_timeout(Duration::from_millis(50));

    let case = case_with_intent("as-6-outer", "read the config", None);
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

// ── T061 / AS-6.4 ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn filter_with_no_match_returns_none() {
    // Judge should never be called because the filter doesn't match any call.
    let judge_inner = Arc::new(MockJudge::always_pass());
    let judge: Arc<dyn JudgeClient> = Arc::clone(&judge_inner) as _;
    let evaluator = SemanticToolParameterEvaluator::new(judge);

    let case = case_with_intent("as-6-4", "read config for project-alpha", Some("read_file"));
    // Agent only calls `list_dir`, not the targeted `read_file`.
    let invocation = invocation_with(&[("list_dir", serde_json::json!({"path": "."}))]);

    assert!(evaluator.evaluate(&case, &invocation).is_none());
    assert_eq!(
        judge_inner.call_count(),
        0,
        "judge must not be invoked when filter excludes all calls"
    );
}

// ── T062: filter set, partial match — only target judged ──────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn filter_with_partial_match_judges_only_target() {
    // Single verdict queued — if the evaluator judged more than the single
    // target call, the mock would run out and report `Other("exhausted")`.
    let verdict = JudgeVerdict {
        score: 1.0,
        pass: true,
        reason: Some("path matches project-alpha".into()),
        label: Some("equivalent".into()),
    };
    let judge_inner = Arc::new(MockJudge::with_verdicts(vec![verdict]));
    let judge: Arc<dyn JudgeClient> = Arc::clone(&judge_inner) as _;
    let evaluator = SemanticToolParameterEvaluator::new(judge);

    let case = case_with_intent(
        "as-6-partial",
        "read config for project-alpha",
        Some("read_file"),
    );
    // Agent makes a mix: one `list_dir`, one `read_file` (target), one `search`.
    let invocation = invocation_with(&[
        ("list_dir", serde_json::json!({"path": "."})),
        (
            "read_file",
            serde_json::json!({"path": "./project-alpha/config.toml"}),
        ),
        ("search", serde_json::json!({"query": "config"})),
    ]);

    let result = evaluator
        .evaluate(&case, &invocation)
        .expect("evaluator must return a metric for the single matching call");

    assert_eq!(result.evaluator_name, "semantic_tool_parameter");
    assert_eq!(result.score.verdict(), Verdict::Pass);
    assert_eq!(
        judge_inner.call_count(),
        1,
        "judge must be invoked exactly once — only the target call"
    );
    let details = result.details.unwrap();
    assert!(
        details.contains("read_file"),
        "details should reference the judged tool, got: {details}"
    );
    assert!(
        !details.contains("list_dir") && !details.contains("search"),
        "details should not reference filtered-out tools, got: {details}"
    );
}
