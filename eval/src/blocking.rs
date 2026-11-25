//! Bridge from sync evaluator entry points into async work on a multi-thread
//! Tokio runtime.

use std::future::Future;

/// Block the current multi-thread-runtime thread until `future` completes.
///
/// Uses `block_in_place` so the runtime moves other tasks off this thread,
/// then drives the future with `futures::executor::block_on` rather than
/// `Handle::block_on`. The thread may itself be driving an outer
/// `Runtime::block_on` (a `#[tokio::main]` or `#[tokio::test]` body awaiting
/// `EvalRunner::run_set`, say). A nested `Handle::block_on` parks on the same
/// per-thread Tokio parker as that outer call and can consume a wakeup meant
/// for it — the outer future then never re-polls and hangs. The `futures`
/// executor parks on the std thread token instead, so the two never collide,
/// and the future still runs on this thread with its thread-locals (scoped
/// judge cancellation, tracing and OpenTelemetry context) intact.
///
/// Callers must have checked that a multi-thread runtime is current;
/// `block_in_place` panics on a current-thread runtime.
pub(crate) fn block_in_place_on<F: Future>(future: F) -> F::Output {
    tokio::task::block_in_place(|| futures::executor::block_on(future))
}

#[cfg(test)]
#[path = "blocking_tests.rs"]
mod tests;
