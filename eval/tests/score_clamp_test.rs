//! Score-clamp regression test (T166 / FR-021 extension).
//!
//! Asserts that a judge returning an out-of-range score (e.g., `1.3`
//! because the LLM emitted a probability rather than a normalized score)
//! is clamped to `1.0` with a structured `ScoreClamped` detail preserved
//! so downstream consumers can flag the raw value.
//!
//! Covers both over-range (`1.3 → 1.0`) and under-range (`-0.2 → 0.0`)
//! clamps, and verifies the `original` value is preserved verbatim in
//! the structured detail.

#![cfg(all(feature = "judge-core", feature = "evaluator-quality"))]

use std::sync::Arc;

use swink_agent::{Cost, ModelSpec, StopReason, Usage};
use swink_agent_eval::{
    CorrectnessEvaluator, Detail, DetailBuffer, EvalCase, Evaluator, Invocation, JudgeClient,
    JudgeEvaluatorConfig, JudgeRegistry, JudgeVerdict, MockJudge,
};

fn config_with(judge: Arc<dyn JudgeClient>) -> JudgeEvaluatorConfig {
    let registry = JudgeRegistry::builder(judge, "mock-model")
        .build()
        .expect("registry");
    JudgeEvaluatorConfig::default_with(Arc::new(registry))
}

fn case() -> EvalCase {
    EvalCase {
        id: "clamp".into(),
        name: "clamp".into(),
        description: None,
        system_prompt: "agent".into(),
        user_messages: vec!["q".into()],
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
        semantic_tool_selection: false,
        state_capture: None,
    }
}

fn invocation_with(response: &str) -> Invocation {
    use std::time::Duration;
    Invocation {
        turns: vec![],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(1),
        final_response: Some(response.into()),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("t", "m"),
    }
}

fn verdict(score: f64) -> JudgeVerdict {
    JudgeVerdict {
        score,
        pass: true,
        reason: Some("ok".into()),
        label: None,
    }
}

fn first_clamp(details: Option<&str>) -> Option<(f64, f64)> {
    let raw = details?;
    // Each structured detail renders as one JSON line. Find the
    // `score_clamped` record and read its fields.
    for line in raw.lines() {
        if let Ok(detail) = serde_json::from_str::<Detail>(line)
            && let Detail::ScoreClamped { original, clamped } = detail
        {
            return Some((original, clamped));
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_range_score_clamps_to_one_and_records_original() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(1.3)]));
    let evaluator = CorrectnessEvaluator::new(config_with(judge));
    let result = evaluator
        .evaluate(&case(), &invocation_with("response"))
        .expect("result");

    assert!((result.score.value - 1.0).abs() < f64::EPSILON);
    let (original, clamped) = first_clamp(result.details.as_deref())
        .expect("FR-021: a ScoreClamped detail must be present when the judge returns > 1.0");
    assert!((original - 1.3).abs() < f64::EPSILON);
    assert!((clamped - 1.0).abs() < f64::EPSILON);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn under_range_score_clamps_to_zero_and_records_original() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(-0.2)]));
    let evaluator = CorrectnessEvaluator::new(config_with(judge));
    let result = evaluator
        .evaluate(&case(), &invocation_with("response"))
        .expect("result");

    assert!((result.score.value - 0.0).abs() < f64::EPSILON);
    let (original, clamped) = first_clamp(result.details.as_deref())
        .expect("FR-021: a ScoreClamped detail must be present when the judge returns < 0.0");
    assert!((original - -0.2).abs() < f64::EPSILON);
    assert!((clamped - 0.0).abs() < f64::EPSILON);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn in_range_score_does_not_record_clamp_detail() {
    let judge: Arc<dyn JudgeClient> = Arc::new(MockJudge::with_verdicts(vec![verdict(0.75)]));
    let evaluator = CorrectnessEvaluator::new(config_with(judge));
    let result = evaluator
        .evaluate(&case(), &invocation_with("response"))
        .expect("result");

    assert!((result.score.value - 0.75).abs() < f64::EPSILON);
    assert!(
        first_clamp(result.details.as_deref()).is_none(),
        "in-range verdicts must not trigger ScoreClamped"
    );
}

#[test]
fn detail_buffer_round_trips_score_clamp_through_details_string() {
    // Sanity: the Detail surface we rely on in this test file still
    // round-trips verbatim. If the detail-buffer contract regresses the
    // assertions above wouldn't catch it directly.
    let mut buffer = DetailBuffer::new();
    buffer.push(Detail::ScoreClamped {
        original: 1.3,
        clamped: 1.0,
    });
    let rendered = buffer.into_details_string().expect("some");
    let parsed: Detail = serde_json::from_str(rendered.lines().next().unwrap()).unwrap();
    assert!(matches!(parsed, Detail::ScoreClamped { .. }));
}
