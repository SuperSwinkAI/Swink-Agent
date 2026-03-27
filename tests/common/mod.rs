//! Shared test mocks and helpers used across integration test files.
//!
//! Most items are re-exported from `swink_agent::testing`. Only items that are
//! truly unique to the integration-test harness live here directly.

use std::sync::Mutex;

// Re-export the canonical test helpers so existing `use common::*` imports
// continue to work without changes in individual test files.
#[allow(unused_imports)]
pub use swink_agent::testing::{
    EventCollector, MockApiKeyCapturingStreamFn, MockContextCapturingStreamFn, MockFlagStreamFn,
    MockTool, default_convert, default_exhausted_fallback, default_model, error_events,
    event_variant_name, next_response, text_events, text_only_events, text_only_events_multi,
    tool_call_events, tool_call_events_multi, user_msg,
};

#[allow(unused_imports)]
pub use swink_agent::AssistantMessageEvent;

// ─── MockStreamFn (error-fallback scripted stream) ───────────────────────
//
// Integration tests expect an error event when scripted responses are
// exhausted. This is a thin wrapper around `ScriptedStreamFn::with_error_fallback`.

/// A mock `StreamFn` that yields scripted event sequences.
///
/// Returns an error event when all responses have been consumed.
#[allow(dead_code)]
pub struct MockStreamFn {
    pub responses: Mutex<Vec<Vec<AssistantMessageEvent>>>,
}

#[allow(dead_code)]
impl MockStreamFn {
    pub fn new(responses: Vec<Vec<AssistantMessageEvent>>) -> Self {
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
