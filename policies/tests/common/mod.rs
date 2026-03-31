//! Shared test mocks for policy integration tests.
//!
//! Re-exports canonical helpers from `swink_agent::testing` so policy tests
//! avoid duplicating mock definitions.

#[allow(unused_imports)]
pub use swink_agent::testing::{
    MockStreamFn, MockTool, default_model, text_only_events, tool_call_events,
};

#[allow(unused_imports)]
pub use swink_agent::AssistantMessageEvent;
