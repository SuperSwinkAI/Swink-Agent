//! Shared test helpers for local-llm integration tests.

use std::sync::{Arc, Mutex};

use swink_agent_local_llm::{ModelConfig, ProgressCallbackFn, ProgressEvent};

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

    /// Return the number of events collected.
    pub fn len(&self) -> usize {
        self.events.lock().unwrap().len()
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

/// Create a test [`ModelConfig`] with defaults suitable for unit tests.
///
/// Does not actually download anything — just provides valid config values.
pub fn test_model_config() -> ModelConfig {
    ModelConfig {
        repo_id: "test/test-model".to_string(),
        filename: "test.gguf".to_string(),
        gpu_layers: 0,
        context_length: 8192,
        chat_template: None,
    }
}
