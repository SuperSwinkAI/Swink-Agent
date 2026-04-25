//! Cross-cutting panic-isolation regression for v2 evaluators.
//!
//! Covers Spec 023 T078 / FR-014 / SC-008: panics inside custom response
//! matchers, semantic judge calls, and environment-state capture must be
//! converted into failing metrics rather than escaping the runner.

use std::collections::HashSet;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use swink_agent::{
    Agent, AgentOptions, ModelSpec,
    testing::{MockStreamFn, MockTool, text_only_events, tool_call_events},
};
use swink_agent_eval::{
    AgentFactory, EnvironmentState, EvalCase, EvalError, EvalRunner, EvaluatorRegistry,
    JudgeClient, JudgeError, JudgeVerdict, ResponseCriteria, Score, ToolIntent, Verdict,
};

struct PanicFactory {
    responses: Vec<Vec<swink_agent::AssistantMessageEvent>>,
}

impl PanicFactory {
    fn new() -> Self {
        Self {
            responses: vec![
                tool_call_events(
                    "call_1",
                    "read_file",
                    r#"{"path":"./project-alpha/config.toml"}"#,
                ),
                text_only_events("done"),
            ],
        }
    }
}

impl AgentFactory for PanicFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let cancel = CancellationToken::new();
        let stream_fn = Arc::new(MockStreamFn::new(self.responses.clone()));
        let options = AgentOptions::new_simple(
            &case.system_prompt,
            ModelSpec::new("test", "test-model"),
            stream_fn,
        )
        .with_tools(vec![Arc::new(MockTool::new("read_file"))]);
        Ok((Agent::new(options), cancel))
    }
}

struct PanickingJudge;

impl JudgeClient for PanickingJudge {
    fn judge<'a>(&'a self, _prompt: &'a str) -> swink_agent_eval::JudgeFuture<'a> {
        Box::pin(async move { panic!("panicking judge") })
    }
}

fn panic_case() -> EvalCase {
    EvalCase {
        id: "panic-isolation".into(),
        name: "Panic isolation".into(),
        description: None,
        system_prompt: "You are a test agent.".into(),
        user_messages: vec!["Read the project-alpha config.".into()],
        expected_trajectory: None,
        expected_response: Some(ResponseCriteria::Custom(Arc::new(|_: &str| -> Score {
            panic!("panicking response closure");
        }))),
        expected_assertion: None,
        expected_interactions: None,
        few_shot_examples: vec![],
        budget: None,
        evaluators: vec![],
        metadata: serde_json::Value::Null,
        attachments: vec![],
        session_id: None,
        expected_environment_state: Some(vec![EnvironmentState {
            name: "created_file".into(),
            state: serde_json::json!("out.md"),
        }]),
        expected_tool_intent: Some(ToolIntent {
            intent: "read config for project-alpha".into(),
            tool_name: Some("read_file".into()),
        }),
        semantic_tool_selection: true,
        state_capture: Some(Arc::new(|_| -> Vec<EnvironmentState> {
            panic!("panicking state capture");
        })),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panics_become_failing_metrics_and_do_not_escape_runner() {
    let registry = EvaluatorRegistry::with_defaults_and_judge(Arc::new(PanickingJudge));
    let runner = EvalRunner::new(registry);
    let factory = PanicFactory::new();

    let result = runner.run_case(&panic_case(), &factory).await.unwrap();

    assert_eq!(result.verdict, Verdict::Fail);

    let failed_metric_names: HashSet<&str> = result
        .metric_results
        .iter()
        .filter(|metric| metric.score.verdict() == Verdict::Fail)
        .map(|metric| metric.evaluator_name.as_str())
        .collect();
    assert_eq!(
        failed_metric_names,
        HashSet::from([
            "response",
            "semantic_tool_selection",
            "semantic_tool_parameter",
            "environment_state",
        ]),
    );

    let response = result
        .metric_results
        .iter()
        .find(|metric| metric.evaluator_name == "response")
        .expect("response metric should exist");
    assert!(
        response
            .details
            .as_deref()
            .is_some_and(|details| details.contains("panicking response closure")),
        "response metric should preserve the panic context"
    );

    let env_state = result
        .metric_results
        .iter()
        .find(|metric| metric.evaluator_name == "environment_state")
        .expect("environment_state metric should exist");
    assert!(
        env_state
            .details
            .as_deref()
            .is_some_and(|details| details.contains("panicking state capture")),
        "environment_state metric should preserve the panic context"
    );

    for evaluator_name in ["semantic_tool_selection", "semantic_tool_parameter"] {
        let metric = result
            .metric_results
            .iter()
            .find(|metric| metric.evaluator_name == evaluator_name)
            .unwrap_or_else(|| panic!("{evaluator_name} metric should exist"));
        assert!(
            metric
                .details
                .as_deref()
                .is_some_and(|details| details.contains("panicking judge")),
            "{evaluator_name} should preserve the judge panic context"
        );
    }
}
