#![cfg(feature = "judge-core")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use swink_agent::{Cost, ModelSpec, StopReason, Usage};
use swink_agent_eval::{
    EvalCase, FewShotExample, Invocation, JudgePromptTemplate, MinijinjaTemplate, PromptContext,
    PromptError, PromptFamily, PromptTemplateRegistry,
};

fn base_case() -> EvalCase {
    EvalCase {
        id: "case-1".to_string(),
        name: "Case One".to_string(),
        description: None,
        system_prompt: "Answer accurately.".to_string(),
        user_messages: vec!["What is 2+2?".to_string()],
        expected_trajectory: None,
        expected_response: None,
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
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
        final_response: Some("4".to_string()),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "judge-target"),
    }
}

fn context() -> PromptContext {
    PromptContext::new(Arc::new(base_case()), Arc::new(invocation()))
}

#[test]
fn minijinja_template_renders_case_invocation_and_examples() {
    let template = MinijinjaTemplate::new(
        "correctness_v0",
        PromptFamily::Quality,
        "Case={{ case.name }} Actual={{ invocation.final_response }} Example={{ few_shot_examples[0].input }}",
    )
    .unwrap();
    let ctx = context().with_few_shot_examples(vec![FewShotExample {
        input: "1+1".to_string(),
        expected: "2".to_string(),
        reasoning: Some("arithmetic".to_string()),
    }]);

    let rendered = template.render(&ctx).unwrap();

    assert!(rendered.contains("Case=Case One"));
    assert!(rendered.contains("Actual=4"));
    assert!(rendered.contains("Example=1+1"));
}

#[test]
fn minijinja_template_rejects_unknown_root_variable_at_construction() {
    let err = MinijinjaTemplate::new("bad_v0", PromptFamily::Quality, "Missing {{ expected }}")
        .expect_err("unknown root variable should fail construction");

    assert!(matches!(
        err,
        PromptError::MissingVariable { name } if name == "expected"
    ));
}

#[test]
fn minijinja_template_supports_custom_namespace() {
    let template =
        MinijinjaTemplate::new("custom_v0", PromptFamily::Agent, "Topic={{ custom.topic }}")
            .unwrap();
    let mut custom = HashMap::new();
    custom.insert("topic".to_string(), serde_json::json!("refunds"));
    let ctx = context().with_custom(custom);

    assert_eq!(template.render(&ctx).unwrap(), "Topic=refunds");
}

#[test]
fn registry_rejects_duplicate_versions() {
    let template: Arc<dyn JudgePromptTemplate> = Arc::new(
        MinijinjaTemplate::new("quality_v0", PromptFamily::Quality, "{{ case.id }}").unwrap(),
    );
    let duplicate: Arc<dyn JudgePromptTemplate> = Arc::new(
        MinijinjaTemplate::new("quality_v0", PromptFamily::Quality, "{{ case.name }}").unwrap(),
    );
    let mut registry = PromptTemplateRegistry::builtin();

    registry.register(Arc::clone(&template)).unwrap();
    let err = registry
        .register(duplicate)
        .expect_err("duplicate versions should be rejected");

    assert!(registry.get("quality_v0").is_some());
    assert!(matches!(
        err,
        PromptError::DuplicateTemplate { version } if version == "quality_v0"
    ));
}
