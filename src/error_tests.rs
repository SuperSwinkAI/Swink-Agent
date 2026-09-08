//! Tests for `error`.
#![cfg(test)]

use super::*;

#[test]
fn agent_error_plugin_display() {
    let err = AgentError::plugin("my-plugin", std::io::Error::other("boom"));
    let msg = format!("{err}");
    assert_eq!(msg, "plugin error (my-plugin)");
}

#[test]
fn plugin_error_not_retryable() {
    let err = AgentError::plugin("test", std::io::Error::other("fail"));
    assert!(!err.is_retryable());
}

#[test]
fn content_filtered_not_retryable() {
    let err = AgentError::ContentFiltered;
    assert!(!err.is_retryable());
    assert_eq!(
        format!("{err}"),
        "content filtered by provider safety policy"
    );
}

#[test]
fn sync_in_async_context_not_retryable() {
    let err = AgentError::SyncInAsyncContext;
    assert!(!err.is_retryable());
    assert!(format!("{err}").contains("sync API"));
}
