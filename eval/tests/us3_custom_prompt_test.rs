#![cfg(all(
    feature = "judge-core",
    feature = "evaluator-quality",
    feature = "evaluator-safety",
    feature = "evaluator-rag",
    feature = "evaluator-agent",
    feature = "evaluator-code",
    feature = "multimodal"
))]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tempfile::tempdir;

use swink_agent::{Cost, ModelSpec, StopReason, Usage};
use swink_agent_eval::{
    CodeLlmJudgeEvaluator, CorrectnessEvaluator, EvalCase, Evaluator, FewShotExample,
    HarmfulnessEvaluator, HelpfulnessEvaluator, ImageSafetyEvaluator, Invocation, JudgeClient,
    JudgeError, JudgeEvaluatorConfig, JudgeRegistry, JudgeVerdict, LanguageDetectionEvaluator,
    MinijinjaTemplate, PIILeakageEvaluator, PromptError, PromptFamily, PromptTemplateRegistry,
    RAGHelpfulnessEvaluator,
};

struct CapturingJudge {
    prompts: Mutex<Vec<String>>,
}

#[async_trait]
impl JudgeClient for CapturingJudge {
    async fn judge(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        Ok(JudgeVerdict {
            score: 0.8,
            pass: true,
            reason: Some("captured".into()),
            label: None,
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
