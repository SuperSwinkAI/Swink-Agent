//! Shared test mocks and helpers used across integration test files.
//!
//! Most items are re-exported from `swink_agent::testing`. Only items that are
//! truly unique to the integration-test harness live here directly.

// Re-export the canonical test helpers so existing `use common::*` imports
// continue to work without changes in individual test files.
#[allow(unused_imports)]
pub use swink_agent::testing::{
    EventCollector, MockApiKeyCapturingStreamFn, MockContextCapturingStreamFn, MockFlagStreamFn,
    MockStreamFn, MockTool, default_convert, default_exhausted_fallback, default_model,
    error_events, event_variant_name, next_response, text_events, text_only_events,
    text_only_events_multi, tool_call_events, tool_call_events_multi, user_msg,
};

#[allow(unused_imports)]
pub use swink_agent::AssistantMessageEvent;

#[cfg(all(feature = "plugins", feature = "testkit"))]
#[allow(unused_imports)]
pub use swink_agent::testing::MockPlugin;
