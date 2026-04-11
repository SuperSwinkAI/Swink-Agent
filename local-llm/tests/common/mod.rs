//! Shared test helpers for local-llm integration tests.

use std::sync::{Arc, Mutex};

use swink_agent::testing::{
    TestGpu, TestOs, TestRuntimeRequirements, should_run_test, test_runtime,
};
use swink_agent_local_llm::{ProgressCallbackFn, ProgressEvent};

/// Collects [`ProgressEvent`]s emitted during model download/load.
///
/// Use [`progress_collector`] to create a callback and the corresponding
/// event log.
#[derive(Debug, Clone)]
pub struct ProgressCollector {
    events: Arc<Mutex<Vec<ProgressEvent>>>,
}

impl ProgressCollector {
    /// Return all collected events so far.
    pub fn events(&self) -> Vec<ProgressEvent> {
        self.events.lock().unwrap().clone()
    }
}

/// Create a [`ProgressCallbackFn`] and its paired [`ProgressCollector`].
pub fn progress_collector() -> (ProgressCallbackFn, ProgressCollector) {
    let events: Arc<Mutex<Vec<ProgressEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = Arc::clone(&events);
    let cb: ProgressCallbackFn = Arc::new(move |event| {
        events_clone.lock().unwrap().push(event);
    });
    let collector = ProgressCollector { events };
    (cb, collector)
}

/// Return `true` when the current host can run direct Gemma 4 local tests.
///
/// Gemma 4 direct inference is only practical when the crate is compiled with
/// a supported GPU backend and the corresponding hardware is present.
pub fn require_gemma4_local_runtime() -> bool {
    if cfg!(feature = "metal") {
        let runtime = test_runtime();
        if runtime.arch != "aarch64" {
            eprintln!(
                "skipping: Gemma 4 local tests require Apple Silicon for the Metal backend (detected arch: {})",
                runtime.arch
            );
            return false;
        }

        return should_run_test(
            TestRuntimeRequirements::new()
                .with_os(TestOs::MacOs)
                .with_gpu(TestGpu::AppleMetal),
        );
    }

    if cfg!(any(feature = "cuda", feature = "cudnn")) {
        return should_run_test(TestRuntimeRequirements::new().with_gpu(TestGpu::Nvidia));
    }

    eprintln!(
        "skipping: Gemma 4 local tests require the crate to be built with `metal`, `cuda`, or `cudnn`"
    );
    false
}
