//! Shared test mocks for policy integration tests.
//!
//! Re-exports canonical helpers from `swink_agent::testing` so policy tests
//! avoid duplicating mock definitions.

use std::sync::Mutex;

#[allow(unused_imports)]
pub use swink_agent::testing::{
    MockTool, default_exhausted_fallback, default_model, next_response, text_only_events,
    tool_call_events,
};

#[allow(unused_imports)]
pub use swink_agent::AssistantMessageEvent;

// ─── MockStreamFn (error-fallback scripted stream) ───────────────────────
//
// Policy tests expect an error event when scripted responses are exhausted.

/// A mock `StreamFn` that yields scripted event sequences.
///
/// Returns an error event when all responses have been consumed.
#[allow(dead_code)]
pub struct MockStreamFn {
    pub responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
}

#[allow(dead_code)]
impl MockStreamFn {
    pub const fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            responses: Mutex::new(responses),
        }
    }
}

impl swink_agent::StreamFn for MockStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a swink_agent::ModelSpec,
        _context: &'a swink_agent::AgentContext,
        _options: &'a swink_agent::StreamOptions,
        _cancellation_token: tokio_util::sync::CancellationToken,
    ) -> std::pin::Pin<
        Box<dyn futures::Stream<Item = swink_agent::AssistantMessageEvent> + Send + 'a>,
    > {
        let events = next_response(&self.responses, default_exhausted_fallback());
        Box::pin(futures::stream::iter(events))
    }
}
