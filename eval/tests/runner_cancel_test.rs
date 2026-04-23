//! US2 / T099: `EvalRunner::with_cancellation` cooperative cancellation.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use swink_agent::{Agent, AgentOptions, ModelSpec, testing::SimpleMockStreamFn};
use swink_agent_eval::{AgentFactory, EvalCase, EvalError, EvalRunner, EvalSet};

mod common;

struct BlockingFactory {
    started: Arc<AtomicUsize>,
    delay: Duration,
}

impl BlockingFactory {
    fn new(delay: Duration) -> Self {
        Self {
            started: Arc::new(AtomicUsize::new(0)),
            delay,
        }
    }
}

impl AgentFactory for BlockingFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
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
    let factory = BlockingFactory::new(Duration::from_millis(0));
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
    let factory = BlockingFactory::new(Duration::from_millis(40));
    let token = CancellationToken::new();
    let runner = EvalRunner::with_defaults()
        .with_parallelism(1)
        .with_cancellation(token.clone());
    let set = eval_set(&["a", "b", "c", "d", "e", "f", "g", "h"]);

    let handle = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(60)).await;
        token.cancel();
    });
    let result = runner.run_set(&set, &factory).await.unwrap();
    let _ = handle.await;

    assert_eq!(result.case_results.len(), 8);
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
    let factory = BlockingFactory::new(Duration::from_millis(0));
    let result = EvalRunner::with_defaults()
        .run_set(&eval_set(&["a"]), &factory)
        .await
        .unwrap();
    assert_eq!(result.case_results.len(), 1);
}
