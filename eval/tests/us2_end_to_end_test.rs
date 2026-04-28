//! US2 / T103: end-to-end assertion of SC-002 + SC-003.
//!
//! 20-case suite / parallelism 4 / num_runs 3 / cache hit → second-run
//! wall-clock ≤ 50% of first-run (jitter-tolerant headline bound; the
//! load-bearing guarantee is agent_invocation_count == 0).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use swink_agent::{Agent, AgentOptions, ModelSpec, testing::SimpleMockStreamFn};
use swink_agent_eval::{
    AgentFactory, EvalCase, EvalError, EvalRunner, EvalSet, EvaluationDataStore, EvaluatorRegistry,
    LocalFileTaskResultStore,
};

mod common;

struct SlowFactory {
    invocations: Arc<AtomicUsize>,
    delay: Duration,
}

impl SlowFactory {
    fn new(delay: Duration) -> Self {
        Self {
            invocations: Arc::new(AtomicUsize::new(0)),
            delay,
        }
    }
}

impl AgentFactory for SlowFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        let options = AgentOptions::new_simple(
            &case.system_prompt,
            ModelSpec::new("test", "test-model"),
            Arc::new(SimpleMockStreamFn::new(vec!["ok".to_string()])),
        );
        Ok((Agent::new(options), CancellationToken::new()))
    }
    fn agent_model(&self, _: &EvalCase) -> Option<String> {
        Some("test/test-model".into())
    }
    fn tool_set_hash(&self, _: &EvalCase) -> Option<String> {
        Some("no-tools".into())
    }
}

fn build_set(count: usize) -> EvalSet {
    EvalSet {
        id: "us2-e2e".into(),
        name: "US2 e2e".into(),
        description: None,
        cases: (0..count)
            .map(|i| common::make_case(&format!("case-{i:02}")))
            .collect(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sc_002_and_sc_003_cache_hit_amortizes_wall_clock() {
    let dir = TempDir::new().unwrap();
    let store: Arc<dyn EvaluationDataStore> = Arc::new(LocalFileTaskResultStore::new(dir.path()));
    let set = build_set(20);

    // First run: warm cache.
    let f1 = SlowFactory::new(Duration::from_millis(15));
    let runner1 = EvalRunner::new(EvaluatorRegistry::new())
        .with_parallelism(4)
        .with_num_runs(3)
        .with_cache(Arc::clone(&store));
    let start = Instant::now();
    let r1 = runner1.run_set(&set, &f1).await.unwrap();
    let first = start.elapsed();
    assert_eq!(r1.case_results.len(), 20);
    assert_eq!(f1.invocations.load(Ordering::SeqCst), 20);
    assert_eq!(runner1.agent_invocation_count(), 20);

    // Second run: pure cache hit.
    let f2 = SlowFactory::new(Duration::from_millis(15));
    let runner2 = EvalRunner::new(EvaluatorRegistry::new())
        .with_parallelism(4)
        .with_num_runs(3)
        .with_cache(Arc::clone(&store));
    let start = Instant::now();
    let r2 = runner2.run_set(&set, &f2).await.unwrap();
    let second = start.elapsed();

    assert_eq!(r2.case_results.len(), 20);
    assert_eq!(
        f2.invocations.load(Ordering::SeqCst),
        0,
        "SC-003: zero agent invocations on cache-hit re-run"
    );
    assert_eq!(runner2.agent_invocation_count(), 0);
    assert!(
        second * 2 <= first,
        "second-run wall-clock {second:?} should be ≤ 50% of first {first:?}"
    );
}
