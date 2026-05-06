//! US2 / T092: `EvalRunner::with_num_runs` variance-recording tests.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::Stream;
use futures::stream;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    Agent, AgentContext, AgentOptions, AssistantMessageEvent, ModelSpec, StreamFn, StreamOptions,
    testing::SimpleMockStreamFn,
};
use swink_agent_eval::{
    AgentFactory, EvalCase, EvalError, EvalMetricResult, EvalRunner, EvalSet, Evaluator,
    EvaluatorRegistry, Invocation, Score,
};
#[cfg(feature = "judge-core")]
use swink_agent_eval::{
    JudgeClient, JudgeEvaluatorConfig, JudgeFuture, JudgeRegistry, JudgeVerdict, PromptContext,
    evaluate_with_builtin,
};

mod common;

struct SequenceEvaluator {
    name: &'static str,
    seq: Mutex<VecDeque<f64>>,
}

impl SequenceEvaluator {
    fn new(name: &'static str, sequence: Vec<f64>) -> Self {
        Self {
            name,
            seq: Mutex::new(sequence.into_iter().collect()),
        }
    }
}

impl Evaluator for SequenceEvaluator {
    fn name(&self) -> &'static str {
        self.name
    }
    fn evaluate(&self, _c: &EvalCase, _i: &Invocation) -> Option<EvalMetricResult> {
        let v = self.seq.lock().unwrap().pop_front().unwrap_or(0.0);
        Some(EvalMetricResult {
            evaluator_name: self.name.to_string(),
            score: Score::new(v, 0.5),
            details: None,
        })
    }
}

struct CallCountingEvaluator {
    calls: Arc<AtomicUsize>,
}

impl Evaluator for CallCountingEvaluator {
    fn name(&self) -> &'static str {
        "call_counter"
    }

    fn evaluate(&self, _c: &EvalCase, _i: &Invocation) -> Option<EvalMetricResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Some(EvalMetricResult {
            evaluator_name: self.name().to_string(),
            score: Score::pass(),
            details: None,
        })
    }
}

struct CountingFactory {
    invocations: Arc<AtomicUsize>,
}

impl CountingFactory {
    fn new() -> Self {
        Self {
            invocations: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl AgentFactory for CountingFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        let options = AgentOptions::new_simple(
            &case.system_prompt,
            ModelSpec::new("test", "test-model"),
            Arc::new(SimpleMockStreamFn::new(vec!["ok".to_string()])),
        );
        Ok((Agent::new(options), CancellationToken::new()))
    }
}

struct CancelsRunnerStreamFn {
    runner_cancel: CancellationToken,
}

impl StreamFn for CancelsRunnerStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> std::pin::Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        self.runner_cancel.cancel();
        Box::pin(stream::pending())
    }
}

struct CancellingFactory {
    runner_cancel: CancellationToken,
}

impl AgentFactory for CancellingFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let options = AgentOptions::new_simple(
            &case.system_prompt,
            ModelSpec::new("test", "test-model"),
            Arc::new(CancelsRunnerStreamFn {
                runner_cancel: self.runner_cancel.clone(),
            }),
        );
        Ok((Agent::new(options), CancellationToken::new()))
    }
}

#[cfg(feature = "judge-core")]
struct RunnerCancellingJudge {
    runner_cancel: CancellationToken,
}

#[cfg(feature = "judge-core")]
impl JudgeClient for RunnerCancellingJudge {
    fn judge<'a>(&'a self, _prompt: &'a str) -> JudgeFuture<'a> {
        let runner_cancel = self.runner_cancel.clone();
        Box::pin(async move {
            runner_cancel.cancel();
            tokio::task::yield_now().await;
            Ok(JudgeVerdict {
                score: 1.0,
                pass: true,
                reason: Some("would pass without runner cancellation".to_string()),
                label: None,
            })
        })
    }
}

#[cfg(feature = "judge-core")]
struct JudgeBackedEvaluator {
    config: JudgeEvaluatorConfig,
}

#[cfg(feature = "judge-core")]
impl Evaluator for JudgeBackedEvaluator {
    fn name(&self) -> &'static str {
        "runner_judge"
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let ctx = PromptContext::new(Arc::new(case.clone()), Arc::new(invocation.clone()));
        Some(evaluate_with_builtin(
            self.name(),
            "helpfulness_v0",
            &self.config,
            &ctx,
        ))
    }
}

fn single_case_set() -> EvalSet {
    EvalSet {
        id: "nr".into(),
        name: "nr".into(),
        description: None,
        cases: vec![common::make_case("c1")],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn num_runs_three_yields_samples_with_variance() {
    let mut registry = EvaluatorRegistry::new();
    registry.register(SequenceEvaluator::new("seq", vec![1.0, 0.5, 0.0]));
    let result = EvalRunner::new(registry)
        .with_num_runs(3)
        .run_set(&single_case_set(), &CountingFactory::new())
        .await
        .unwrap();

    let metric = &result.case_results[0].metric_results[0];
    let details = metric.details.as_ref().expect("details recorded");
    assert!(details.contains("num_runs=3"), "{details}");
    assert!(details.contains("mean=0.5"), "{details}");
    assert!(details.contains("std_dev=0.4"), "{details}");
    assert!((metric.score.value - 0.5).abs() < 1e-6);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn num_runs_single_is_backwards_compatible() {
    let mut registry = EvaluatorRegistry::new();
    registry.register(SequenceEvaluator::new("seq", vec![1.0, 0.0]));
    let result = EvalRunner::new(registry)
        .with_num_runs(1)
        .run_set(&single_case_set(), &CountingFactory::new())
        .await
        .unwrap();
    assert!((result.case_results[0].metric_results[0].score.value - 1.0).abs() < 1e-6);
}

#[test]
fn num_runs_zero_panics() {
    let result = std::panic::catch_unwind(|| {
        let _ = EvalRunner::with_defaults().with_num_runs(0);
    });
    assert!(result.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn num_runs_reuses_single_invocation() {
    let mut registry = EvaluatorRegistry::new();
    registry.register(SequenceEvaluator::new("seq", vec![0.9; 5]));
    let factory = CountingFactory::new();
    let calls = Arc::clone(&factory.invocations);
    let runner = EvalRunner::new(registry).with_num_runs(5);
    let _ = runner.run_set(&single_case_set(), &factory).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(runner.agent_invocation_count(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_single_run_dispatch_records_failure_metric_without_evaluating() {
    let cancel = CancellationToken::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = EvaluatorRegistry::new();
    registry.register(CallCountingEvaluator {
        calls: Arc::clone(&calls),
    });

    let result = EvalRunner::new(registry)
        .with_cancellation(cancel.clone())
        .run_set(
            &single_case_set(),
            &CancellingFactory {
                runner_cancel: cancel,
            },
        )
        .await
        .unwrap();

    let case_result = &result.case_results[0];
    assert_eq!(case_result.verdict, swink_agent_eval::Verdict::Fail);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let cancelled_metric = case_result
        .metric_results
        .iter()
        .find(|metric| metric.evaluator_name == "cancelled")
        .expect("cancellation during single-run dispatch should record a metric");
    assert_eq!(
        cancelled_metric.score.verdict(),
        swink_agent_eval::Verdict::Fail
    );
    assert!(
        cancelled_metric
            .details
            .as_deref()
            .is_some_and(|details| details.contains("before evaluator dispatch")),
        "unexpected cancellation details: {cancelled_metric:?}"
    );
}

#[cfg(feature = "judge-core")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runner_cancellation_reaches_judge_backed_evaluators() {
    let cancel = CancellationToken::new();
    let judge = Arc::new(RunnerCancellingJudge {
        runner_cancel: cancel.clone(),
    });
    let judge_registry = JudgeRegistry::builder(judge as Arc<dyn JudgeClient>, "mock-model")
        .build()
        .unwrap();
    let mut registry = EvaluatorRegistry::new();
    registry.register(JudgeBackedEvaluator {
        config: JudgeEvaluatorConfig::default_with(Arc::new(judge_registry)),
    });

    let result = EvalRunner::new(registry)
        .with_cancellation(cancel)
        .run_set(&single_case_set(), &CountingFactory::new())
        .await
        .unwrap();

    let metric = &result.case_results[0].metric_results[0];
    assert_eq!(metric.evaluator_name, "runner_judge");
    assert_eq!(metric.score.verdict(), swink_agent_eval::Verdict::Fail);
    assert!(
        metric
            .details
            .as_deref()
            .is_some_and(|details| details.contains("cancelled")),
        "expected judge dispatch to observe runner cancellation: {metric:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_num_runs_dispatch_records_failure_metric() {
    let cancel = CancellationToken::new();
    let result = EvalRunner::with_defaults()
        .with_num_runs(3)
        .with_cancellation(cancel.clone())
        .run_set(
            &single_case_set(),
            &CancellingFactory {
                runner_cancel: cancel,
            },
        )
        .await
        .unwrap();

    let case_result = &result.case_results[0];
    assert_eq!(case_result.verdict, swink_agent_eval::Verdict::Fail);
    let cancelled_metric = case_result
        .metric_results
        .iter()
        .find(|metric| metric.evaluator_name == "cancelled")
        .expect("cancellation during num_runs dispatch should record a metric");
    assert_eq!(
        cancelled_metric.score.verdict(),
        swink_agent_eval::Verdict::Fail
    );
    assert!(
        cancelled_metric
            .details
            .as_deref()
            .is_some_and(|details| details.contains("multi-run evaluator dispatch")),
        "unexpected cancellation details: {cancelled_metric:?}"
    );
}
