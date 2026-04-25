#![cfg(all(
    feature = "judge-core",
    feature = "evaluator-quality",
    feature = "evaluator-safety",
    feature = "evaluator-rag",
    feature = "evaluator-agent",
    feature = "evaluator-code",
    feature = "multimodal"
))]

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::tempdir;

use swink_agent::{Cost, ModelSpec, StopReason, Usage};
use swink_agent_eval::{
    AgentToneEvaluator, Assertion, AssertionKind, Attachment, CodeInjectionEvaluator,
    CodeLlmJudgeEvaluator, CoherenceEvaluator, ConcisenessEvaluator, CorrectnessEvaluator,
    EvalCase, EvalMetricResult, Evaluator, EvaluatorRegistry, ExpectedToolCall, FairnessEvaluator,
    FaithfulnessEvaluator, FewShotExample, GoalSuccessRateEvaluator, HallucinationEvaluator,
    HarmfulnessEvaluator, HelpfulnessEvaluator, ImageSafetyEvaluator, InteractionExpectation,
    InteractionsEvaluator, Invocation, JudgeClient, JudgeError, JudgeEvaluatorConfig,
    JudgeRegistry, JudgeVerdict, KnowledgeRetentionEvaluator, LanguageDetectionEvaluator,
    LazinessEvaluator, MinijinjaTemplate, PIIClass, PIILeakageEvaluator, PerceivedErrorEvaluator,
    PlanAdherenceEvaluator, PromptError, PromptFamily, PromptInjectionEvaluator,
    PromptTemplateRegistry, RAGGroundednessEvaluator, RAGHelpfulnessEvaluator,
    RAGRetrievalRelevanceEvaluator, ResponseRelevanceEvaluator, TaskCompletionEvaluator,
    ToxicityEvaluator, TrajectoryAccuracyEvaluator, TrajectoryAccuracyWithRefEvaluator,
    UserSatisfactionEvaluator,
};

struct CapturingJudge {
    prompts: Mutex<Vec<String>>,
}

const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x62, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

impl JudgeClient for CapturingJudge {
    fn judge<'a>(&'a self, prompt: &'a str) -> swink_agent_eval::JudgeFuture<'a> {
        Box::pin(async move {
            self.prompts.lock().unwrap().push(prompt.to_string());
            Ok(JudgeVerdict {
                score: 0.8,
                pass: true,
                reason: Some("captured".into()),
                label: None,
            })
        })
    }
}

fn registry(judge: Arc<dyn JudgeClient>) -> Arc<JudgeRegistry> {
    Arc::new(
        JudgeRegistry::builder(judge, "mock-model")
            .build()
            .expect("registry builds"),
    )
}

fn config(judge: Arc<dyn JudgeClient>) -> JudgeEvaluatorConfig {
    JudgeEvaluatorConfig::default_with(registry(judge))
}

fn base_case() -> EvalCase {
    EvalCase {
        id: "case-1".into(),
        name: "Case One".into(),
        description: None,
        system_prompt: "original system prompt".into(),
        user_messages: vec!["what is two plus two?".into()],
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

fn invocation() -> Invocation {
    Invocation {
        turns: vec![],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(1),
        final_response: Some("4".into()),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "judge-target"),
    }
}

fn fully_populated_case() -> EvalCase {
    let mut case = base_case();
    case.expected_assertion = Some(Assertion {
        description: "The task should be completed.".into(),
        kind: AssertionKind::GoalCompleted,
    });
    case.expected_trajectory = Some(vec![ExpectedToolCall {
        tool_name: "search".into(),
        arguments: Some(serde_json::json!({ "query": "two plus two" })),
    }]);
    case.expected_interactions = Some(vec![InteractionExpectation {
        from: "planner".into(),
        to: "worker".into(),
        description: "delegates the task".into(),
    }]);
    case.few_shot_examples = vec![FewShotExample {
        input: "retrieved evidence".into(),
        expected: "ground truth".into(),
        reasoning: Some("because evidence".into()),
    }];
    case.attachments = vec![Attachment::Base64 {
        mime: "image/png".into(),
        bytes: TINY_PNG.to_vec(),
    }];
    case
}

fn assert_prompt_version(metric: &EvalMetricResult, version: &str) {
    let details = metric.details.as_deref().expect("details present");
    assert!(
        details.contains(version),
        "expected prompt version {version} in details for {} but got: {details}",
        metric.evaluator_name
    );
}

fn register_all_judge_backed_evaluators(
    registry: &mut EvaluatorRegistry,
    judge_client: &Arc<dyn JudgeClient>,
    eval_root: &Path,
) {
    let cfg = || config(Arc::clone(judge_client));

    registry
        .add(HelpfulnessEvaluator::new(cfg()))
        .expect("helpfulness registered");
    registry
        .add(CorrectnessEvaluator::new(cfg()))
        .expect("correctness registered");
    registry
        .add(ConcisenessEvaluator::new(cfg()))
        .expect("conciseness registered");
    registry
        .add(CoherenceEvaluator::new(cfg()))
        .expect("coherence registered");
    registry
        .add(ResponseRelevanceEvaluator::new(cfg()))
        .expect("response relevance registered");
    registry
        .add(HallucinationEvaluator::new(cfg()))
        .expect("hallucination registered");
    registry
        .add(FaithfulnessEvaluator::new(cfg()))
        .expect("faithfulness registered");
    registry
        .add(PlanAdherenceEvaluator::new(cfg()))
        .expect("plan adherence registered");
    registry
        .add(LazinessEvaluator::new(cfg()))
        .expect("laziness registered");
    registry
        .add(GoalSuccessRateEvaluator::new(cfg()))
        .expect("goal success rate registered");
    registry
        .add(HarmfulnessEvaluator::new(cfg()))
        .expect("harmfulness registered");
    registry
        .add(ToxicityEvaluator::new(cfg()))
        .expect("toxicity registered");
    registry
        .add(FairnessEvaluator::new(cfg()))
        .expect("fairness registered");
    registry
        .add(PIILeakageEvaluator::with_classes(
            cfg(),
            vec![PIIClass::Email, PIIClass::Phone, PIIClass::Ssn],
        ))
        .expect("pii leakage registered");
    registry
        .add(PromptInjectionEvaluator::new(cfg()))
        .expect("prompt injection registered");
    registry
        .add(CodeInjectionEvaluator::new(cfg()))
        .expect("code injection registered");
    registry
        .add(RAGGroundednessEvaluator::new(cfg()))
        .expect("rag groundedness registered");
    registry
        .add(RAGRetrievalRelevanceEvaluator::new(cfg()))
        .expect("rag retrieval relevance registered");
    registry
        .add(RAGHelpfulnessEvaluator::new(cfg()))
        .expect("rag helpfulness registered");
    registry
        .add(TrajectoryAccuracyEvaluator::new(cfg()))
        .expect("trajectory accuracy registered");
    registry
        .add(TrajectoryAccuracyWithRefEvaluator::new(cfg()))
        .expect("trajectory accuracy with ref registered");
    registry
        .add(TaskCompletionEvaluator::new(cfg()))
        .expect("task completion registered");
    registry
        .add(UserSatisfactionEvaluator::new(cfg()))
        .expect("user satisfaction registered");
    registry
        .add(AgentToneEvaluator::new(cfg()))
        .expect("agent tone registered");
    registry
        .add(KnowledgeRetentionEvaluator::new(cfg()))
        .expect("knowledge retention registered");
    registry
        .add(LanguageDetectionEvaluator::new(cfg()))
        .expect("language detection registered");
    registry
        .add(PerceivedErrorEvaluator::new(cfg()))
        .expect("perceived error registered");
    registry
        .add(InteractionsEvaluator::new(cfg()))
        .expect("interactions registered");
    registry
        .add(CodeLlmJudgeEvaluator::new(cfg()))
        .expect("code llm judge registered");
    registry
        .add(ImageSafetyEvaluator::new(cfg(), eval_root))
        .expect("image safety registered");
}

fn expected_prompt_versions() -> HashMap<&'static str, &'static str> {
    HashMap::from([
        ("helpfulness", "helpfulness_v0"),
        ("correctness", "correctness_v0"),
        ("conciseness", "conciseness_v0"),
        ("coherence", "coherence_v0"),
        ("response_relevance", "response_relevance_v0"),
        ("hallucination", "hallucination_v0"),
        ("faithfulness", "faithfulness_v0"),
        ("plan_adherence", "plan_adherence_v0"),
        ("laziness", "laziness_v0"),
        ("goal_success_rate", "goal_success_rate_v0"),
        ("harmfulness", "harmfulness_v0"),
        ("toxicity", "toxicity_v0"),
        ("fairness", "fairness_v0"),
        ("pii_leakage", "pii_leakage_v0"),
        ("prompt_injection", "prompt_injection_v0"),
        ("code_injection", "code_injection_v0"),
        ("rag_groundedness", "rag_groundedness_v0"),
        ("rag_retrieval_relevance", "rag_retrieval_relevance_v0"),
        ("rag_helpfulness", "rag_helpfulness_v0"),
        ("trajectory_accuracy", "trajectory_accuracy_v0"),
        (
            "trajectory_accuracy_with_ref",
            "trajectory_accuracy_with_ref_v0",
        ),
        ("task_completion", "task_completion_v0"),
        ("user_satisfaction", "user_satisfaction_v0"),
        ("agent_tone", "agent_tone_v0"),
        ("knowledge_retention", "knowledge_retention_v0"),
        ("language_detection", "language_detection_v0"),
        ("perceived_error", "perceived_error_v0"),
        ("interactions", "interactions_v0"),
        ("code_llm_judge", "code_llm_judge_v0"),
        ("image_safety", "image_safety_v0"),
    ])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn custom_prompt_override_uses_builder_supplied_render_context() {
    let judge = Arc::new(CapturingJudge {
        prompts: Mutex::new(Vec::new()),
    });
    let prompt = Arc::new(
        MinijinjaTemplate::new(
            "correctness_v1",
            PromptFamily::Quality,
            "SP={{ case.system_prompt }}|EX={{ few_shot_examples[0].input }}|R={{ custom.use_reasoning }}|K={{ custom.feedback_key }}|T={{ custom.output_schema.type }}|A={{ invocation.final_response }}",
        )
        .expect("template compiles"),
    );

    let evaluator = CorrectnessEvaluator::new(config(judge.clone()))
        .with_prompt(prompt)
        .with_few_shot(vec![FewShotExample {
            input: "few-shot input".into(),
            expected: "few-shot expected".into(),
            reasoning: Some("few-shot reasoning".into()),
        }])
        .with_system_prompt("override system prompt")
        .with_output_schema(serde_json::json!({ "type": "object" }))
        .with_use_reasoning(false)
        .with_feedback_key("correctness_custom");

    let result = evaluator
        .evaluate(&base_case(), &invocation())
        .expect("correctness evaluator emits a result");

    let details = result.details.expect("details populated");
    assert!(details.contains("correctness_v1"));

    let prompts = judge.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1);
    let rendered = &prompts[0];
    assert!(rendered.contains("SP=override system prompt"));
    assert!(rendered.contains("EX=few-shot input"));
    assert!(rendered.contains("R=false"));
    assert!(rendered.contains("K=correctness_custom"));
    assert!(rendered.contains("T=object"));
    assert!(rendered.contains("A=4"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompt_version_bump_is_explicit_opt_in() {
    let builtin_result = CorrectnessEvaluator::new(config(Arc::new(CapturingJudge {
        prompts: Mutex::new(Vec::new()),
    })))
    .evaluate(&base_case(), &invocation())
    .expect("builtin correctness emits a result");
    assert!(
        builtin_result
            .details
            .expect("details present")
            .contains("correctness_v0")
    );

    let custom = Arc::new(
        MinijinjaTemplate::new(
            "correctness_v1",
            PromptFamily::Quality,
            "Case={{ case.id }} Actual={{ invocation.final_response }}",
        )
        .expect("template compiles"),
    );
    let bumped_result = CorrectnessEvaluator::new(config(Arc::new(CapturingJudge {
        prompts: Mutex::new(Vec::new()),
    })))
    .with_prompt(custom)
    .evaluate(&base_case(), &invocation())
    .expect("custom correctness emits a result");
    assert!(
        bumped_result
            .details
            .expect("details present")
            .contains("correctness_v1")
    );
    assert!(
        PromptTemplateRegistry::builtin()
            .get("correctness_v0")
            .is_some()
    );
}

#[test]
fn missing_variable_is_rejected_at_template_construction() {
    let err = MinijinjaTemplate::new(
        "correctness_broken_v0",
        PromptFamily::Quality,
        "Missing {{ expected }}",
    )
    .expect_err("unknown root variable should fail construction");

    assert!(matches!(
        err,
        PromptError::MissingVariable { name } if name == "expected"
    ));
}

#[test]
fn builder_surface_is_exposed_for_representative_judge_backed_evaluators() {
    let prompt = Arc::new(
        MinijinjaTemplate::new("surface_v1", PromptFamily::Quality, "{{ case.id }}")
            .expect("template compiles"),
    );
    let few_shot = vec![FewShotExample {
        input: "one".into(),
        expected: "two".into(),
        reasoning: None,
    }];
    let schema = serde_json::json!({ "type": "object" });

    let quality = HelpfulnessEvaluator::new(config(Arc::new(CapturingJudge {
        prompts: Mutex::new(Vec::new()),
    })))
    .with_prompt(prompt.clone())
    .with_few_shot(few_shot.clone())
    .with_system_prompt("quality system")
    .with_output_schema(schema.clone())
    .with_use_reasoning(false)
    .with_feedback_key("quality.feedback");
    assert_eq!(
        quality.config().system_prompt.as_deref(),
        Some("quality system")
    );
    assert_eq!(quality.config().few_shot_examples, few_shot);

    let safety = HarmfulnessEvaluator::new(config(Arc::new(CapturingJudge {
        prompts: Mutex::new(Vec::new()),
    })))
    .with_prompt(prompt.clone())
    .with_feedback_key("safety.feedback");
    assert_eq!(
        safety.config().feedback_key.as_deref(),
        Some("safety.feedback")
    );

    let pii = PIILeakageEvaluator::new(config(Arc::new(CapturingJudge {
        prompts: Mutex::new(Vec::new()),
    })))
    .with_prompt(prompt.clone())
    .with_output_schema(schema.clone());
    assert_eq!(pii.config().output_schema.as_ref(), Some(&schema));

    let rag = RAGHelpfulnessEvaluator::new(config(Arc::new(CapturingJudge {
        prompts: Mutex::new(Vec::new()),
    })))
    .with_prompt(prompt.clone())
    .with_use_reasoning(false);
    assert!(!rag.config().use_reasoning);

    let agent = LanguageDetectionEvaluator::new(config(Arc::new(CapturingJudge {
        prompts: Mutex::new(Vec::new()),
    })))
    .with_prompt(prompt.clone())
    .with_feedback_key("agent.feedback");
    assert_eq!(
        agent.config().feedback_key.as_deref(),
        Some("agent.feedback")
    );

    let code = CodeLlmJudgeEvaluator::new(config(Arc::new(CapturingJudge {
        prompts: Mutex::new(Vec::new()),
    })))
    .with_prompt(prompt.clone())
    .with_system_prompt("code system");
    assert_eq!(code.config().system_prompt.as_deref(), Some("code system"));

    let tmp = tempdir().expect("tempdir");
    let multimodal = ImageSafetyEvaluator::new(
        config(Arc::new(CapturingJudge {
            prompts: Mutex::new(Vec::new()),
        })),
        tmp.path(),
    )
    .with_prompt(prompt)
    .with_feedback_key("image.feedback");
    assert_eq!(
        multimodal.config().feedback_key.as_deref(),
        Some("image.feedback")
    );
}

#[test]
fn every_judge_backed_evaluator_records_prompt_version() {
    let judge = Arc::new(CapturingJudge {
        prompts: Mutex::new(Vec::new()),
    });
    let judge_client: Arc<dyn JudgeClient> = judge.clone();
    let tmp = tempdir().expect("tempdir");

    let mut registry = EvaluatorRegistry::new();
    register_all_judge_backed_evaluators(&mut registry, &judge_client, tmp.path());

    let results = registry.evaluate(&fully_populated_case(), &invocation());
    assert_eq!(results.len(), 30);

    let expected_versions = expected_prompt_versions();

    for metric in &results {
        let expected = expected_versions
            .get(metric.evaluator_name.as_str())
            .unwrap_or_else(|| panic!("unexpected metric {}", metric.evaluator_name));
        assert_prompt_version(metric, expected);
    }

    assert_eq!(
        judge.prompts.lock().unwrap().len(),
        expected_versions.len(),
        "every judge-backed evaluator should dispatch exactly once"
    );
}

struct NamedEvaluator<E> {
    name: &'static str,
    inner: E,
}

impl<E> NamedEvaluator<E> {
    fn new(name: &'static str, inner: E) -> Self {
        Self { name, inner }
    }
}

impl<E> Evaluator for NamedEvaluator<E>
where
    E: Evaluator,
{
    fn name(&self) -> &'static str {
        self.name
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        self.inner.evaluate(case, invocation).map(|mut metric| {
            metric.evaluator_name = self.name.to_string();
            metric
        })
    }
}

#[test]
fn registry_can_distinguish_builtin_and_custom_prompt_versions() {
    let judge = Arc::new(CapturingJudge {
        prompts: Mutex::new(Vec::new()),
    });
    let judge_client: Arc<dyn JudgeClient> = judge.clone();
    let custom_prompt = Arc::new(
        MinijinjaTemplate::new(
            "correctness_v1",
            PromptFamily::Quality,
            "Case={{ case.id }} Actual={{ invocation.final_response }}",
        )
        .expect("template compiles"),
    );

    let mut registry = EvaluatorRegistry::new();
    registry
        .add(CorrectnessEvaluator::new(config(Arc::clone(&judge_client))))
        .expect("builtin correctness registered");
    registry
        .add(NamedEvaluator::new(
            "correctness_v1_custom",
            CorrectnessEvaluator::new(config(judge_client)).with_prompt(custom_prompt),
        ))
        .expect("custom correctness registered");

    let mut builtin_case = base_case();
    builtin_case.evaluators = vec!["correctness".into()];
    let builtin_results = registry.evaluate(&builtin_case, &invocation());
    assert_eq!(builtin_results.len(), 1);
    assert_eq!(builtin_results[0].evaluator_name, "correctness");
    assert_prompt_version(&builtin_results[0], "correctness_v0");

    let mut custom_case = base_case();
    custom_case.evaluators = vec!["correctness_v1_custom".into()];
    let custom_results = registry.evaluate(&custom_case, &invocation());
    assert_eq!(custom_results.len(), 1);
    assert_eq!(custom_results[0].evaluator_name, "correctness_v1_custom");
    assert_prompt_version(&custom_results[0], "correctness_v1");

    let prompts = judge.prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert_ne!(builtin_results[0].details, custom_results[0].details);
}
