//! Shared test helpers for adapter integration tests.
//!
//! Provides common utility functions used across multiple adapter test files
//! to avoid duplication of event introspection and mock response builders.

#![allow(dead_code)]

use std::sync::Arc;

use swink_agent::{AgentContext, AssistantMessageEvent, StopReason};
use tokio::sync::Notify;
use wiremock::{Request, Respond, ResponseTemplate};

/// Return a human-readable name for an `AssistantMessageEvent` variant.
pub const fn event_name(event: &AssistantMessageEvent) -> &'static str {
    match event {
        AssistantMessageEvent::Start => "Start",
        AssistantMessageEvent::TextStart { .. } => "TextStart",
        AssistantMessageEvent::TextDelta { .. } => "TextDelta",
        AssistantMessageEvent::TextEnd { .. } => "TextEnd",
        AssistantMessageEvent::ThinkingStart { .. } => "ThinkingStart",
        AssistantMessageEvent::ThinkingDelta { .. } => "ThinkingDelta",
        AssistantMessageEvent::ThinkingEnd { .. } => "ThinkingEnd",
        AssistantMessageEvent::ToolCallStart { .. } => "ToolCallStart",
        AssistantMessageEvent::ToolCallDelta { .. } => "ToolCallDelta",
        AssistantMessageEvent::ToolCallEnd { .. } => "ToolCallEnd",
        AssistantMessageEvent::Done { .. } => "Done",
        AssistantMessageEvent::Error { .. } => "Error",
        _ => "Unknown",
    }
}

/// Extract the error message from the first `Error` event, if any.
pub fn find_error_message(events: &[AssistantMessageEvent]) -> Option<String> {
    events.iter().find_map(|e| match e {
        AssistantMessageEvent::Error { error_message, .. } => Some(error_message.clone()),
        _ => None,
    })
}

/// Extract the structured `error_kind` from the first `Error` event.
///
/// Returns `None` when no `Error` event is present, `Some(None)` when the
/// first `Error` event carries no structured kind, and `Some(Some(kind))`
/// when the adapter attached a `StreamErrorKind`.
#[allow(clippy::option_option)] // outer Option = event presence, inner = structured kind
pub fn find_error_kind(
    events: &[AssistantMessageEvent],
) -> Option<Option<swink_agent::StreamErrorKind>> {
    events.iter().find_map(|e| match e {
        AssistantMessageEvent::Error { error_kind, .. } => Some(*error_kind),
        _ => None,
    })
}

/// Extract the `retry_after` hint from the first `Error` event, if any.
///
/// Returns `None` both when there is no `Error` event and when the `Error`
/// event carries no retry-after hint — callers that need to distinguish
/// those cases should use `find_error_message` to confirm an `Error` event
/// exists first.
pub fn find_retry_after(events: &[AssistantMessageEvent]) -> Option<std::time::Duration> {
    events.iter().find_map(|e| match e {
        AssistantMessageEvent::Error { retry_after, .. } => *retry_after,
        _ => None,
    })
}

/// Extract the stop reason from the first `Done` event, if any.
pub fn extract_stop_reason(events: &[AssistantMessageEvent]) -> Option<StopReason> {
    events.iter().find_map(|e| match e {
        AssistantMessageEvent::Done { stop_reason, .. } => Some(*stop_reason),
        _ => None,
    })
}

/// Build a `ResponseTemplate` with SSE content type.
pub fn sse_response(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("Content-Type", "text/event-stream")
        .set_body_string(body.to_owned())
}

/// Return a responder and notifier that fires when the mock handles a request.
pub fn notify_on_request(response: ResponseTemplate) -> (impl Respond, Arc<Notify>) {
    let request_seen = Arc::new(Notify::new());
    let responder_request_seen = Arc::clone(&request_seen);

    let responder = move |_request: &Request| {
        responder_request_seen.notify_one();
        response.clone()
    };

    (responder, request_seen)
}

/// Build a minimal `AgentContext` for testing.
pub fn test_context() -> AgentContext {
    AgentContext::new("You are a test assistant.", Vec::new(), Vec::new())
}
