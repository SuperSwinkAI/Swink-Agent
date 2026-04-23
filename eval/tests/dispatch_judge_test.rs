//! Integration coverage for the shared `dispatch_judge` helper (T056).

#![cfg(feature = "judge-core")]

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use swink_agent::{Cost, ModelSpec, StopReason, Usage};
use swink_agent_eval::{
    Detail, EvalCase, Invocation, JudgeClient, JudgeError, JudgeEvaluatorConfig,
    JudgePromptTemplate, JudgeRegistry, JudgeVerdict, MinijinjaTemplate, PromptContext,
    PromptFamily, PromptTemplateRegistry, dispatch_judge, finish_metric_result,
};

struct CannedJudge {
    score: f64,
    reason: Option<String>,
    prompts: Mutex<Vec<String>>,
}

#[async_trait]
impl JudgeClient for CannedJudge {
    async fn judge(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError> {
        self.prompts.lock().unwrap().push(prompt.to_string());
        Ok(JudgeVerdict {
            score: self.score,
            pass: (0.5..=1.0).contains(&self.score),
            reason: self.reason.clone(),
            label: None,
        })
    }
}

fn base_case() -> EvalCase {
    EvalCase {
        id: "case-1".into(),
        name: "Case One".into(),
        description: None,
        system_prompt: "answer".into(),
        user_messages: vec!["hi".into()],
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

fn base_invocation() -> Invocation {
    Invocation {
        turns: vec![],
        total_usage: Usage::default(),
        total_cost: Cost::default(),
        total_duration: Duration::from_millis(1),
        final_response: Some("42".into()),
        stop_reason: StopReason::Stop,
        model: ModelSpec::new("test", "judge-target"),
    }
}

fn registry_for(score: f64) -> (Arc<JudgeRegistry>, Arc<CannedJudge>) {
    let judge = Arc::new(CannedJudge {
        score,
        reason: Some("ok".into()),
        prompts: Mutex::new(Vec::new()),
    });
    let registry = JudgeRegistry::builder(judge.clone() as Arc<dyn JudgeClient>, "mock-model")
        .build()
        .expect("registry builds");
    (Arc::new(registry), judge)
}

fn builtin_correctness() -> Arc<dyn JudgePromptTemplate> {
    PromptTemplateRegistry::builtin()
        .get("correctness_v0")
        .expect("correctness_v0 is a built-in template")
}

#[tokio::test]
async fn dispatch_records_prompt_version_for_builtin() {
    let (registry, _) = registry_for(0.75);
    let config = JudgeEvaluatorConfig::default_with(registry);
    let ctx = PromptContext::new(Arc::new(base_case()), Arc::new(base_invocation()));

    let outcome = dispatch_judge(&config, builtin_correctness(), &ctx)
        .await
        .expect("dispatch succeeds");

    let recorded = outcome
        .details
        .entries()
        .iter()
        .find_map(|d| match d {
            Detail::PromptVersion { version } => Some(version.clone()),
            _ => None,
        })
        .expect("prompt_version recorded");
    assert_eq!(recorded, "correctness_v0");
    assert!((outcome.score.value - 0.75).abs() < f64::EPSILON);
}

#[tokio::test]
async fn dispatch_clamps_out_of_range_score_with_detail() {
    let (registry, _) = registry_for(1.3);
    let config = JudgeEvaluatorConfig::default_with(registry);
    let ctx = PromptContext::new(Arc::new(base_case()), Arc::new(base_invocation()));

    let outcome = dispatch_judge(&config, builtin_correctness(), &ctx)
        .await
        .expect("dispatch succeeds");

    assert!((outcome.score.value - 1.0).abs() < f64::EPSILON);
    let (original, clamped) = outcome
        .details
        .entries()
        .iter()
        .find_map(|d| match d {
            Detail::ScoreClamped { original, clamped } => Some((*original, *clamped)),
            _ => None,
        })
        .expect("ScoreClamped detail is present");
    assert!((original - 1.3).abs() < f64::EPSILON);
    assert!((clamped - 1.0).abs() < f64::EPSILON);
}

#[tokio::test]
async fn dispatch_prefers_config_override_template() {
    let (registry, judge) = registry_for(0.5);
    let custom: Arc<dyn JudgePromptTemplate> = Arc::new(
        MinijinjaTemplate::new(
            "correctness_v1",
            PromptFamily::Quality,
            "custom Case={{ case.id }}",
        )
        .unwrap(),
    );
    let config = JudgeEvaluatorConfig::default_with(registry).with_template(custom);
    let ctx = PromptContext::new(Arc::new(base_case()), Arc::new(base_invocation()));

    let outcome = dispatch_judge(&config, builtin_correctness(), &ctx)
        .await
        .expect("dispatch succeeds");

    let recorded = outcome
        .details
        .entries()
        .iter()
        .find_map(|d| match d {
            Detail::PromptVersion { version } => Some(version.clone()),
            _ => None,
        })
        .expect("prompt_version recorded");
    assert_eq!(recorded, "correctness_v1");
    let seen = judge.prompts.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].starts_with("custom Case=case-1"));
}

#[tokio::test]
async fn finish_metric_result_round_trips_details_and_reason() {
    let (registry, _) = registry_for(1.5);
    let config = JudgeEvaluatorConfig::default_with(registry);
    let ctx = PromptContext::new(Arc::new(base_case()), Arc::new(base_invocation()));

    let outcome = dispatch_judge(&config, builtin_correctness(), &ctx)
        .await
        .expect("dispatch succeeds");
    let metric = finish_metric_result("CorrectnessEvaluator", outcome);

    assert_eq!(metric.evaluator_name, "CorrectnessEvaluator");
    assert!((metric.score.value - 1.0).abs() < f64::EPSILON);
    let details = metric.details.expect("details populated");
    // Each line is a JSON object.
    let lines: Vec<&str> = details.lines().collect();
    assert!(lines.iter().any(|l| l.contains("\"prompt_version\"")));
    assert!(lines.iter().any(|l| l.contains("\"score_clamped\"")));
    assert!(lines.iter().any(|l| l.contains("\"note\"")));
}
