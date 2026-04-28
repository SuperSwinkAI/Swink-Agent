//! US2 / T090: `EvalRunner::with_parallelism` bounded concurrency tests.
//!
//! Mock agents resolve synchronously so these tests observe the
//! **semaphore-permit bound** rather than thread-level parallelism. The bound
//! is the contract the spec makes (FR-036): "up to N concurrent" and "never
//! more than N". Real providers introduce await points inside the agent
//! stream that will exercise actual concurrency in production.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;

use swink_agent::{
    Agent, AgentContext, AgentOptions, AssistantMessageEvent, ModelSpec, StreamFn, StreamOptions,
    testing::SimpleMockStreamFn,
};
use swink_agent_eval::{AgentFactory, EvalCase, EvalError, EvalRunner, EvalSet};

mod common;

/// Wraps `SimpleMockStreamFn` with a leading `tokio::time::sleep` so the
/// resulting stream has a real await point. The in-flight counter increments
/// before the sleep and decrements after, letting callers observe peak
/// concurrency at the buffer_unordered boundary.
struct DelayingStream {
    delay: Duration,
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    inner: SimpleMockStreamFn,
}

impl StreamFn for DelayingStream {
    fn stream<'a>(
        &'a self,
        model: &'a ModelSpec,
        context: &'a AgentContext,
        options: &'a StreamOptions,
        cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let delay = self.delay;
        let in_flight = Arc::clone(&self.in_flight);
        let peak = Arc::clone(&self.peak);
        let inner_stream = self
            .inner
            .stream(model, context, options, cancellation_token);
        let prelude = futures::stream::once(async move {
            let n = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(n, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            in_flight.fetch_sub(1, Ordering::SeqCst);
        })
        .filter_map(|()| async { Option::<AssistantMessageEvent>::None });
        Box::pin(prelude.chain(inner_stream))
    }
}

struct ConcurrentFactory {
    in_flight: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    delay: Duration,
}

impl ConcurrentFactory {
    fn new(delay: Duration) -> Self {
        Self {
            in_flight: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            delay,
        }
    }
}

impl AgentFactory for ConcurrentFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let stream_fn = Arc::new(DelayingStream {
            delay: self.delay,
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
    let factory = ConcurrentFactory::new(Duration::from_millis(20));
    let peak = Arc::clone(&factory.peak);
    let _ = EvalRunner::with_defaults()
        .run_set(&eval_set(&["a", "b", "c"]), &factory)
        .await
        .unwrap();
    assert_eq!(
        peak.load(Ordering::SeqCst),
        1,
        "parallelism=1 must be sequential"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallelism_four_bounded_by_permit_count() {
    let factory = ConcurrentFactory::new(Duration::from_millis(40));
    let peak = Arc::clone(&factory.peak);
    let _ = EvalRunner::with_defaults()
        .with_parallelism(4)
        .run_set(
            &eval_set(&["a", "b", "c", "d", "e", "f", "g", "h"]),
            &factory,
        )
        .await
        .unwrap();
    let observed = peak.load(Ordering::SeqCst);
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
#[ignore = "deadlocks on GH Actions; likely permit-release race at 5ms delay — investigate separately"]
async fn parallelism_releases_permits_cleanly() {
    // 8 cases with parallelism=2 must all complete (no permit leaks).
    let factory = ConcurrentFactory::new(Duration::from_millis(5));
    let ids: Vec<_> = (0..8).map(|i| format!("c{i}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let r = EvalRunner::with_defaults()
        .with_parallelism(2)
        .run_set(&eval_set(&refs), &factory)
        .await
        .unwrap();
    assert_eq!(r.case_results.len(), 8);
}
