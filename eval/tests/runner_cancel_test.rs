//! US2 / T099: `EvalRunner::with_cancellation` cooperative cancellation.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};

use tokio_util::sync::CancellationToken;

use swink_agent::{Agent, AgentOptions, ModelSpec, testing::SimpleMockStreamFn};
use swink_agent_eval::{AgentFactory, EvalCase, EvalError, EvalRunner, EvalSet};

mod common;

struct BlockingFactory {
    started: Arc<AtomicUsize>,
    start_gate: Option<Arc<StartGate>>,
}

struct StartGate {
    started_tx: mpsc::Sender<()>,
    release_rx: Mutex<mpsc::Receiver<()>>,
}

impl BlockingFactory {
    fn new() -> Self {
        Self {
            started: Arc::new(AtomicUsize::new(0)),
            start_gate: None,
        }
    }

    fn with_start_gate(started_tx: mpsc::Sender<()>, release_rx: mpsc::Receiver<()>) -> Self {
        Self {
            started: Arc::new(AtomicUsize::new(0)),
            start_gate: Some(Arc::new(StartGate {
                started_tx,
                release_rx: Mutex::new(release_rx),
            })),
        }
    }
}

impl AgentFactory for BlockingFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.start_gate {
            let _ = gate.started_tx.send(());
            gate.release_rx
                .lock()
                .expect("start gate lock poisoned")
                .recv()
                .expect("start gate release sender dropped");
        }
        let options = AgentOptions::new_simple(
            &case.system_prompt,
            ModelSpec::new("test", "test-model"),
            Arc::new(SimpleMockStreamFn::new(vec!["ok".to_string()])),
        );
        Ok((Agent::new(options), CancellationToken::new()))
    }
}

fn eval_set(ids: &[&str]) -> EvalSet {
    EvalSet {
        id: "cancel-suite".into(),
        name: "cancel suite".into(),
        description: None,
        cases: ids.iter().map(|id| common::make_case(id)).collect(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pre_cancelled_token_skips_every_case() {
    let factory = BlockingFactory::new();
    let token = CancellationToken::new();
    token.cancel();
    let result = EvalRunner::with_defaults()
        .with_cancellation(token)
        .run_set(&eval_set(&["a", "b", "c"]), &factory)
        .await
        .unwrap();
    assert_eq!(result.case_results.len(), 3);
    for cr in &result.case_results {
        assert!(
            cr.metric_results
                .iter()
                .any(|m| m.evaluator_name == "cancelled"),
            "{} missing cancelled metric",
            cr.case_id
        );
    }
    assert_eq!(factory.started.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mid_run_cancellation_returns_partial_results() {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let factory = BlockingFactory::with_start_gate(started_tx, release_rx);
    let started = Arc::clone(&factory.started);
    let token = CancellationToken::new();
    let runner = EvalRunner::with_defaults()
        .with_parallelism(1)
        .with_cancellation(token.clone());
    let set = eval_set(&["a", "b", "c", "d", "e", "f", "g", "h"]);

    let run_handle = tokio::spawn(async move { runner.run_set(&set, &factory).await.unwrap() });
    tokio::task::spawn_blocking(move || {
        started_rx
            .recv()
            .expect("factory should signal first case start");
    })
    .await
    .expect("start waiter should not panic");
    token.cancel();
    release_tx
        .send(())
        .expect("factory should still be waiting");
    let result = run_handle.await.expect("runner task should not panic");

    assert_eq!(result.case_results.len(), 8);
    assert_eq!(started.load(Ordering::SeqCst), 1);
    let cancelled = result
        .case_results
        .iter()
        .filter(|r| {
            r.metric_results
                .iter()
                .any(|m| m.evaluator_name == "cancelled")
        })
        .count();
    assert!(cancelled > 0, "at least one case should be cancelled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_is_optional_by_default() {
    let factory = BlockingFactory::new();
    let result = EvalRunner::with_defaults()
        .run_set(&eval_set(&["a"]), &factory)
        .await
        .unwrap();
    assert_eq!(result.case_results.len(), 1);
}
