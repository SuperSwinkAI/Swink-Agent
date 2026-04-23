//! US2 / T092: `EvalRunner::with_num_runs` variance-recording tests.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use swink_agent::{Agent, AgentOptions, ModelSpec, testing::SimpleMockStreamFn};
use swink_agent_eval::{
    AgentFactory, EvalCase, EvalError, EvalMetricResult, EvalRunner, EvalSet, Evaluator,
    EvaluatorRegistry, Invocation, Score,
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
