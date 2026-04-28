//! Integration tests for `EvalRunner` — suite execution, empty suites, and error continuation.

mod common;

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use swink_agent::{
    Agent, AgentMessage, AgentOptions, ContentBlock, LlmMessage, ModelSpec, UserMessage,
    testing::SimpleMockStreamFn,
};
use swink_agent_eval::{
    AgentFactory, EvalCase, EvalError, EvalMetricResult, EvalRunner, EvalSet, Evaluator,
    EvaluatorRegistry, Invocation, Score, Verdict,
};

/// Factory that creates agents with a deterministic mock stream returning the
/// given tokens as a text-only response.
struct MockFactory {
    tokens: Vec<String>,
}

impl MockFactory {
    fn new(tokens: Vec<&str>) -> Self {
        Self {
            tokens: tokens.into_iter().map(String::from).collect(),
        }
    }
}

impl AgentFactory for MockFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let cancel = CancellationToken::new();
        let stream_fn = Arc::new(SimpleMockStreamFn::new(self.tokens.clone()));
        let model = ModelSpec::new("test", "test-model");
        let options = AgentOptions::new_simple(&case.system_prompt, model, stream_fn);
        let agent = Agent::new(options);
        Ok((agent, cancel))
    }
}

/// Factory that always fails to create an agent.
struct FailingFactory;

impl AgentFactory for FailingFactory {
    fn create_agent(&self, _case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        Err(EvalError::invalid_case("factory forced failure"))
    }
}

struct TrackingFactory {
    tokens: Vec<String>,
    observed_cancel: Arc<Mutex<Option<CancellationToken>>>,
}

impl TrackingFactory {
    fn new(tokens: Vec<&str>) -> Self {
        Self {
            tokens: tokens.into_iter().map(String::from).collect(),
            observed_cancel: Arc::new(Mutex::new(None)),
        }
    }

    fn observed_cancel(&self) -> CancellationToken {
        self.observed_cancel
            .lock()
            .expect("observed cancel lock should not poison")
            .clone()
            .expect("factory should record a cancellation token")
    }
}

impl AgentFactory for TrackingFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let cancel = CancellationToken::new();
        *self
            .observed_cancel
            .lock()
            .expect("observed cancel lock should not poison") = Some(cancel.clone());
        let stream_fn = Arc::new(SimpleMockStreamFn::new(self.tokens.clone()));
        let model = ModelSpec::new("test", "test-model");
        let options = AgentOptions::new_simple(&case.system_prompt, model, stream_fn);
        let agent = Agent::new(options);
        Ok((agent, cancel))
    }
}

struct BusyFactory {
    observed_cancel: Arc<Mutex<Option<CancellationToken>>>,
}

impl BusyFactory {
    fn new() -> Self {
        Self {
            observed_cancel: Arc::new(Mutex::new(None)),
        }
    }

    fn observed_cancel(&self) -> CancellationToken {
        self.observed_cancel
            .lock()
            .expect("observed cancel lock should not poison")
            .clone()
            .expect("factory should record a cancellation token")
    }
}

impl AgentFactory for BusyFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let cancel = CancellationToken::new();
        *self
            .observed_cancel
            .lock()
            .expect("observed cancel lock should not poison") = Some(cancel.clone());
        let stream_fn = Arc::new(SimpleMockStreamFn::new(vec!["busy".to_string()]));
        let model = ModelSpec::new("test", "test-model");
        let options = AgentOptions::new_simple(&case.system_prompt, model, stream_fn);
        let mut agent = Agent::new(options);
        let stream = agent
            .prompt_stream(vec![AgentMessage::Llm(LlmMessage::User(UserMessage {
                content: vec![ContentBlock::Text {
                    text: "already running".to_string(),
                }],
                timestamp: swink_agent::now_timestamp(),
                cache_hint: None,
            }))])
            .expect("precondition stream should start");
        std::mem::forget(stream);
        Ok((agent, cancel))
    }
}

struct PanicEvaluator;

impl Evaluator for PanicEvaluator {
    fn name(&self) -> &'static str {
        "panic_eval"
    }

    fn evaluate(&self, _case: &EvalCase, _invocation: &Invocation) -> Option<EvalMetricResult> {
        panic!("runner evaluator panic");
    }
}

struct PassingEvaluator;

impl Evaluator for PassingEvaluator {
    fn name(&self) -> &'static str {
        "pass_eval"
    }

    fn evaluate(&self, _case: &EvalCase, _invocation: &Invocation) -> Option<EvalMetricResult> {
        Some(EvalMetricResult {
            evaluator_name: self.name().to_string(),
            score: Score::pass(),
            details: None,
        })
    }
}

fn make_case(id: &str) -> EvalCase {
    EvalCase {
        id: id.to_string(),
        name: format!("Case {id}"),
        description: None,
        system_prompt: "You are a test agent.".to_string(),
        user_messages: vec!["Hello".to_string()],
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

#[tokio::test]
async fn run_set_multi_case_produces_per_case_scores() {
    let factory = MockFactory::new(vec!["Hello", " world"]);
    let runner = EvalRunner::with_defaults();
    let eval_set = EvalSet {
        id: "suite".to_string(),
        name: "Test Suite".to_string(),
        description: None,
        cases: vec![make_case("a"), make_case("b"), make_case("c")],
    };

    let result = runner.run_set(&eval_set, &factory).await.unwrap();
    assert_eq!(result.case_results.len(), 3);
    assert_eq!(result.summary.total_cases, 3);
    assert_eq!(
        result.summary.passed + result.summary.failed,
        3,
        "all cases should be accounted for"
    );
    // Each case should have its ID preserved.
    let ids: Vec<_> = result
        .case_results
        .iter()
        .map(|r| r.case_id.as_str())
        .collect();
    assert_eq!(ids, vec!["a", "b", "c"]);
}

#[tokio::test]
async fn empty_suite_returns_empty_report() {
    let factory = MockFactory::new(vec!["Hello"]);
    let runner = EvalRunner::with_defaults();
    let eval_set = EvalSet {
        id: "empty".to_string(),
        name: "Empty Suite".to_string(),
        description: None,
        cases: vec![],
    };

    let result = runner.run_set(&eval_set, &factory).await.unwrap();
    assert!(result.case_results.is_empty());
    assert_eq!(result.summary.total_cases, 0);
    assert_eq!(result.summary.passed, 0);
    assert_eq!(result.summary.failed, 0);
}

#[tokio::test]
async fn case_failure_recorded_and_suite_continues() {
    // FailingFactory causes every case to error, but run_set should record
    // each as a failed result and continue rather than aborting.
    let runner = EvalRunner::with_defaults();
    let eval_set = EvalSet {
        id: "suite".to_string(),
        name: "Failing Suite".to_string(),
        description: None,
        cases: vec![make_case("a"), make_case("b"), make_case("c")],
    };

    let result = runner.run_set(&eval_set, &FailingFactory).await.unwrap();

    // All three cases should be present as failed results.
    assert_eq!(
        result.case_results.len(),
        3,
        "all cases should have results"
    );
    assert_eq!(result.summary.total_cases, 3);
    assert_eq!(result.summary.failed, 3);
    assert_eq!(result.summary.passed, 0);

    for case_result in &result.case_results {
        assert_eq!(case_result.verdict, Verdict::Fail);
        // The error evaluator should have recorded the error message.
        assert!(!case_result.metric_results.is_empty());
        let error_metric = case_result
            .metric_results
            .iter()
            .find(|m| m.evaluator_name == "error")
            .expect("should have error metric");
        assert!(
            error_metric
                .details
                .as_ref()
                .unwrap()
                .contains("factory forced failure")
        );
    }
}

#[tokio::test]
async fn mixed_success_and_failure() {
    // Create a factory that succeeds, and test that when mixed with a failing
    // factory scenario, the results reflect both.
    let factory = MockFactory::new(vec!["Hello"]);
    let runner = EvalRunner::with_defaults();

    // Run a single successful case.
    let eval_set = EvalSet {
        id: "mixed".to_string(),
        name: "Mixed Suite".to_string(),
        description: None,
        cases: vec![make_case("success")],
    };

    let result = runner.run_set(&eval_set, &factory).await.unwrap();
    assert_eq!(result.case_results.len(), 1);
    // A case with no expected trajectory/response and only evaluators that do
    // not apply must not pass vacuously.
    assert_eq!(result.summary.passed, 0);
    assert_eq!(result.summary.failed, 1);
    assert_eq!(result.case_results[0].verdict, Verdict::Fail);
    assert!(
        result.case_results[0]
            .metric_results
            .iter()
            .any(|metric| metric.evaluator_name == "no_applicable_evaluators")
    );
}

#[tokio::test]
async fn run_case_fails_when_no_evaluator_produces_metric() {
    let factory = MockFactory::new(vec!["Hello"]);
    let runner = EvalRunner::new(EvaluatorRegistry::new());

    let result = runner
        .run_case(&make_case("empty-metrics"), &factory)
        .await
        .unwrap();

    assert_eq!(result.verdict, Verdict::Fail);
    assert_eq!(result.metric_results.len(), 1);
    let metric = &result.metric_results[0];
    assert_eq!(metric.evaluator_name, "no_applicable_evaluators");
    assert_eq!(metric.score.verdict(), Verdict::Fail);
}

#[tokio::test]
async fn run_case_cancels_factory_token_after_success() {
    let factory = TrackingFactory::new(vec!["Hello"]);
    let runner = EvalRunner::with_defaults();

    let result = runner.run_case(&make_case("cancel-ok"), &factory).await;

    assert!(result.is_ok(), "run_case should succeed");
    assert!(
        factory.observed_cancel().is_cancelled(),
        "runner should cancel the factory token after a completed case"
    );
}

#[tokio::test]
async fn run_case_cancels_factory_token_when_prompt_stream_fails() {
    let factory = BusyFactory::new();
    let runner = EvalRunner::with_defaults();

    runner
        .run_case(&make_case("cancel-err"), &factory)
        .await
        .expect_err("busy agent should reject prompt_stream");
    assert!(
        factory.observed_cancel().is_cancelled(),
        "runner should cancel the factory token on prompt_stream startup failure"
    );
}

#[tokio::test]
async fn panicking_evaluator_records_failure_and_suite_continues() {
    let factory = MockFactory::new(vec!["Hello"]);
    let mut registry = EvaluatorRegistry::new();
    registry.register(PanicEvaluator);
    registry.register(PassingEvaluator);
    let runner = EvalRunner::new(registry);
    let eval_set = EvalSet {
        id: "panic-safe".to_string(),
        name: "Panic Safe Suite".to_string(),
        description: None,
        cases: vec![make_case("a"), make_case("b")],
    };

    let result = runner.run_set(&eval_set, &factory).await.unwrap();

    assert_eq!(result.case_results.len(), 2);
    assert_eq!(result.summary.total_cases, 2);
    assert_eq!(result.summary.failed, 2);
    assert_eq!(result.summary.passed, 0);

    for case_result in &result.case_results {
        let panic_metric = case_result
            .metric_results
            .iter()
            .find(|metric| metric.evaluator_name == "panic_eval")
            .expect("panic metric should be recorded");
        assert_eq!(panic_metric.score.verdict(), Verdict::Fail);
        assert!(
            panic_metric
                .details
                .as_deref()
                .is_some_and(|details| details.contains("runner evaluator panic")),
            "panic metric should preserve the panic message"
        );
        assert!(
            case_result
                .metric_results
                .iter()
                .any(|metric| metric.evaluator_name == "pass_eval"),
            "non-panicking evaluators should still run"
        );
    }
}
