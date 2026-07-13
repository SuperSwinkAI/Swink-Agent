//! US2 / T090: `EvalRunner::with_parallelism` bounded concurrency tests.
//!
//! Mock agents resolve synchronously so these tests observe the
//! **semaphore-permit bound** rather than thread-level parallelism. The bound
//! is the contract the spec makes (FR-036): "up to N concurrent" and "never
//! more than N". Real providers introduce await points inside the agent
//! stream that will exercise actual concurrency in production.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use futures::{Stream, StreamExt};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    Agent, AgentContext, AgentOptions, AssistantMessageEvent, ModelSpec, StreamFn, StreamOptions,
    testing::SimpleMockStreamFn,
};
use swink_agent_eval::{AgentFactory, EvalCase, EvalError, EvalRunner, EvalSet};

mod common;

/// Wraps `SimpleMockStreamFn` with a deterministic gate so the resulting
/// stream has a real await point. The in-flight counter increments before the
/// gate and decrements after, letting callers observe peak concurrency at the
/// permit boundary without depending on wall-clock sleeps.
struct GatedStream {
    gate: Arc<GateState>,
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    inner: SimpleMockStreamFn,
}

impl StreamFn for GatedStream {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let gate = Arc::clone(&self.gate);
        let in_flight = Arc::clone(&self.in_flight);
        let peak = Arc::clone(&self.peak);
        let inner_stream = self
            .inner
            .stream(model, context, options, cancellation_token);
        let prelude = futures::stream::once(async move {
            gate.started.fetch_add(1, Ordering::SeqCst);
            let n = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(n, Ordering::SeqCst);
            gate.progress.notify_waiters();
            loop {
                let notified = gate.progress.notified();
                if gate.open.load(Ordering::SeqCst) {
                    break;
                }
                notified.await;
            }
            in_flight.fetch_sub(1, Ordering::SeqCst);
        })
        .filter_map(|()| async { Option::<AssistantMessageEvent>::None });
        Box::pin(prelude.chain(inner_stream))
    }
}

struct GateState {
    open: AtomicBool,
    started: AtomicUsize,
    progress: Notify,
}

impl GateState {
    fn new() -> Self {
        Self {
            open: AtomicBool::new(false),
            started: AtomicUsize::new(0),
            progress: Notify::new(),
        }
    }

    fn release(&self) {
        self.open.store(true, Ordering::SeqCst);
        self.progress.notify_waiters();
    }

    async fn wait_for_started(&self, expected: usize) {
        loop {
            let notified = self.progress.notified();
            if self.started.load(Ordering::SeqCst) >= expected {
                return;
            }
            notified.await;
        }
    }
}

struct ConcurrentFactory {
    gate: Arc<GateState>,
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl ConcurrentFactory {
    fn new() -> Self {
        Self {
            gate: Arc::new(GateState::new()),
            in_flight: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl AgentFactory for ConcurrentFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let stream_fn = Arc::new(GatedStream {
            gate: Arc::clone(&self.gate),
            in_flight: Arc::clone(&self.in_flight),
            peak: Arc::clone(&self.peak),
            inner: SimpleMockStreamFn::new(vec!["ok".into()]),
        });
        let options = AgentOptions::new_simple(
            &case.system_prompt,
            ModelSpec::new("test", "test-model"),
            stream_fn,
        );
        Ok((Agent::new(options), CancellationToken::new()))
    }
}

async fn run_gated_set(parallelism: usize, ids: &[&str]) -> (Arc<ConcurrentFactory>, usize) {
    let factory = Arc::new(ConcurrentFactory::new());
    let peak = Arc::clone(&factory.peak);
    let set = eval_set(ids);
    let runner_factory = Arc::clone(&factory);
    let handle = tokio::spawn(async move {
        EvalRunner::with_defaults()
            .with_parallelism(parallelism)
            .run_set(&set, runner_factory.as_ref())
            .await
            .unwrap()
    });
    factory
        .gate
        .wait_for_started(parallelism.min(ids.len()))
        .await;
    factory.gate.release();
    let result = handle.await.unwrap();
    assert_eq!(result.case_results.len(), ids.len());
    (factory, peak.load(Ordering::SeqCst))
}

fn eval_set(ids: &[&str]) -> EvalSet {
    EvalSet {
        id: "parallelism-suite".into(),
        name: "parallelism suite".into(),
        description: None,
        cases: ids.iter().map(|id| common::make_case(id)).collect(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallelism_one_is_sequential_baseline() {
    let (_factory, observed) = run_gated_set(1, &["a", "b", "c"]).await;
    assert_eq!(observed, 1, "parallelism=1 must be sequential");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallelism_four_bounded_by_permit_count() {
    let (_factory, observed) = run_gated_set(4, &["a", "b", "c", "d", "e", "f", "g", "h"]).await;
    assert!(
        observed <= 4,
        "parallelism=4 must never exceed bound (saw {observed})"
    );
}

#[test]
fn parallelism_zero_panics() {
    let r = std::panic::catch_unwind(|| {
        let _ = EvalRunner::with_defaults().with_parallelism(0);
    });
    assert!(r.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallelism_releases_permits_cleanly() {
    // 8 cases with parallelism=2 must all complete (no permit leaks).
    let ids: Vec<_> = (0..8).map(|i| format!("c{i}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let (_factory, observed) = run_gated_set(2, &refs).await;
    assert!(
        observed <= 2,
        "parallelism=2 must never exceed bound (saw {observed})"
    );
}
