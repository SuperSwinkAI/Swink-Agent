//! US1 / T089: registry wiring + end-to-end integration acceptance.

#![cfg(feature = "all-evaluators")]

use std::sync::Arc;

use serde_json::json;
use tempfile::tempdir;

use swink_agent_eval::{
    Assertion, AssertionKind, Attachment, CodeLlmJudgeEvaluator, CorrectnessEvaluator, EvalCase,
    EvalMetricResult, EvaluatorRegistry, ExactMatchEvaluator, HarmfulnessEvaluator,
    ImageSafetyEvaluator, JsonSchemaEvaluator, JudgeClient, JudgeEvaluatorConfig, JudgeRegistry,
    MockJudge, RAGGroundednessEvaluator, TaskCompletionEvaluator,
};

mod common;

const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x62, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

fn judge_config(judge: Arc<dyn JudgeClient>) -> JudgeEvaluatorConfig {
    let registry = JudgeRegistry::builder(judge, "mock-model")
        .build()
        .expect("judge registry builds");
    JudgeEvaluatorConfig::default_with(Arc::new(registry))
}

fn base_case(id: &str) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        name: format!("Case {id}"),
        description: None,
        system_prompt: "You are a test agent.".to_string(),
        user_messages: vec!["test prompt".to_string()],
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

fn metric<'a>(results: &'a [EvalMetricResult], name: &str) -> &'a EvalMetricResult {
    results
        .iter()
        .find(|result| result.evaluator_name == name)
        .unwrap_or_else(|| panic!("missing metric {name}"))
}

fn assert_judge_metric(results: &[EvalMetricResult], name: &str, prompt_version: &str) {
    let result = metric(results, name);
    assert!(
        !result.details.as_deref().unwrap_or_default().is_empty(),
        "{name} should record details"
    );
    let details = result
        .details
        .as_deref()
        .expect("judge-backed details present");
    assert!(
        details.contains(prompt_version),
        "{name} should record prompt version {prompt_version}, got: {details}"
    );
    assert!(
        details.contains("mock pass"),
        "{name} should preserve the judge reason, got: {details}"
    );
}

fn assert_baseline_results(registry: &EvaluatorRegistry) {
    let mut baseline_case = base_case("baseline");
    baseline_case.evaluators = vec![
        "correctness".into(),
        "harmfulness".into(),
        "exact_match".into(),
        "code_llm_judge".into(),
    ];
    let baseline_invocation = common::mock_invocation_with_response(&[], "expected answer");
    let baseline_results = registry.evaluate(&baseline_case, &baseline_invocation);
    assert_eq!(baseline_results.len(), 4);
    assert!(
        metric(&baseline_results, "exact_match")
            .score
            .verdict()
            .is_pass()
    );
    assert_judge_metric(&baseline_results, "correctness", "correctness_v0");
    assert_judge_metric(&baseline_results, "harmfulness", "harmfulness_v0");
    assert_judge_metric(&baseline_results, "code_llm_judge", "code_llm_judge_v0");
}

fn assert_rag_results(registry: &EvaluatorRegistry) {
    let mut rag_case = base_case("rag");
    rag_case.evaluators = vec!["rag_groundedness".into()];
    rag_case.few_shot_examples = vec![swink_agent_eval::FewShotExample {
        input: "Paris is the capital of France.".into(),
        expected: "retrieved passage".into(),
        reasoning: None,
    }];
    let rag_invocation =
        common::mock_invocation_with_response(&[], "Paris is the capital of France.");
    let rag_results = registry.evaluate(&rag_case, &rag_invocation);
    assert_eq!(rag_results.len(), 1);
    assert_judge_metric(&rag_results, "rag_groundedness", "rag_groundedness_v0");
}

fn assert_agent_results(registry: &EvaluatorRegistry) {
    let mut agent_case = base_case("agent");
    agent_case.evaluators = vec!["task_completion".into()];
    agent_case.expected_assertion = Some(Assertion {
        description: "The task should be completed.".into(),
        kind: AssertionKind::GoalCompleted,
    });
    let agent_invocation = common::mock_invocation_with_response(&[], "The task is complete.");
    let agent_results = registry.evaluate(&agent_case, &agent_invocation);
    assert_eq!(agent_results.len(), 1);
    assert_judge_metric(&agent_results, "task_completion", "task_completion_v0");
}

fn assert_structured_results(registry: &EvaluatorRegistry) {
    let mut structured_case = base_case("structured");
    structured_case.evaluators = vec!["json_schema".into()];
    let structured_invocation =
        common::mock_invocation_with_response(&[], r#"{"answer":"expected answer"}"#);
    let structured_results = registry.evaluate(&structured_case, &structured_invocation);
    assert_eq!(structured_results.len(), 1);
    assert!(
        metric(&structured_results, "json_schema")
            .score
            .verdict()
            .is_pass()
    );
}

fn assert_multimodal_results(registry: &EvaluatorRegistry) {
    let mut multimodal_case = base_case("multimodal");
    multimodal_case.evaluators = vec!["image_safety".into()];
    multimodal_case.attachments = vec![Attachment::Base64 {
        mime: "image/png".into(),
        bytes: TINY_PNG.to_vec(),
    }];
    let multimodal_invocation = common::mock_invocation_with_response(&[], "safe");
    let multimodal_results = registry.evaluate(&multimodal_case, &multimodal_invocation);
    assert_eq!(multimodal_results.len(), 1);
    assert_judge_metric(&multimodal_results, "image_safety", "image_safety_v0");
}

#[test]
fn us1_registry_skips_non_applicable_evaluators() {
    let judge = Arc::new(MockJudge::always_pass());
    let judge_client: Arc<dyn JudgeClient> = judge.clone();
    let eval_root = tempdir().expect("tempdir");

    let mut registry = EvaluatorRegistry::new();
    registry
        .add(CorrectnessEvaluator::new(judge_config(Arc::clone(
            &judge_client,
        ))))
        .expect("correctness registered");
    registry
        .add(RAGGroundednessEvaluator::new(judge_config(Arc::clone(
            &judge_client,
        ))))
        .expect("rag registered");
    registry
        .add(TaskCompletionEvaluator::new(judge_config(Arc::clone(
            &judge_client,
        ))))
        .expect("task completion registered");
    registry
        .add(ImageSafetyEvaluator::new(
            judge_config(judge_client),
            eval_root.path(),
        ))
        .expect("image safety registered");

    let case = base_case("non-applicable");
    let invocation = common::mock_invocation_with_response(&[], "42");

    let results = registry.evaluate(&case, &invocation);
    assert_eq!(results.len(), 1, "only correctness should apply");
    assert_eq!(results[0].evaluator_name, "correctness");
    assert_eq!(judge.call_count(), 1);
}

#[test]
fn us1_registry_exercises_one_evaluator_per_family_via_root_exports() {
    let judge = Arc::new(MockJudge::always_pass());
    let judge_client: Arc<dyn JudgeClient> = judge.clone();
    let eval_root = tempdir().expect("tempdir");

    let mut registry = EvaluatorRegistry::new();
    registry
        .add(CorrectnessEvaluator::new(judge_config(Arc::clone(
            &judge_client,
        ))))
        .expect("correctness registered");
    registry
        .add(HarmfulnessEvaluator::new(judge_config(Arc::clone(
            &judge_client,
        ))))
        .expect("harmfulness registered");
    registry
        .add(RAGGroundednessEvaluator::new(judge_config(Arc::clone(
            &judge_client,
        ))))
        .expect("rag registered");
    registry
        .add(TaskCompletionEvaluator::new(judge_config(Arc::clone(
            &judge_client,
        ))))
        .expect("task completion registered");
    registry
        .add(
            JsonSchemaEvaluator::new(&json!({
                "type": "object",
                "required": ["answer"],
                "properties": { "answer": { "type": "string" } }
            }))
            .expect("json schema evaluator builds"),
        )
        .expect("json schema registered");
    registry
        .add(ExactMatchEvaluator::new("expected answer"))
        .expect("exact match registered");
    registry
        .add(CodeLlmJudgeEvaluator::new(judge_config(Arc::clone(
            &judge_client,
        ))))
        .expect("code llm judge registered");
    registry
        .add(ImageSafetyEvaluator::new(
            judge_config(judge_client),
            eval_root.path(),
        ))
        .expect("image safety registered");

    assert_baseline_results(&registry);
    assert_rag_results(&registry);
    assert_agent_results(&registry);
    assert_structured_results(&registry);
    assert_multimodal_results(&registry);

    assert_eq!(
        judge.call_count(),
        6,
        "quality, safety, rag, agent, code, and multimodal families should each dispatch once"
    );
}
