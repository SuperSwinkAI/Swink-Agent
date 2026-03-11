use agent_harness::HarnessError;
use std::error::Error;
use std::io;

/// Test 1.8 — `HarnessError` variants display meaningful messages.
#[test]
fn display_messages() {
    assert_eq!(
        HarnessError::context_overflow("claude-sonnet-4-6").to_string(),
        "context window overflow for model: claude-sonnet-4-6"
    );
    assert_eq!(
        HarnessError::ModelThrottled.to_string(),
        "model request throttled (rate limited)"
    );
    assert_eq!(
        HarnessError::network(io::Error::new(io::ErrorKind::ConnectionReset, "reset")).to_string(),
        "network error"
    );
    assert_eq!(
        HarnessError::structured_output_failed(3, "schema mismatch").to_string(),
        "structured output failed after 3 attempts: schema mismatch"
    );
    assert_eq!(
        HarnessError::AlreadyRunning.to_string(),
        "agent is already running"
    );
    assert_eq!(
        HarnessError::NoMessages.to_string(),
        "cannot continue with empty message history"
    );
    assert_eq!(
        HarnessError::InvalidContinue.to_string(),
        "cannot continue when last message is an assistant message"
    );
    assert_eq!(
        HarnessError::stream(io::Error::new(io::ErrorKind::InvalidData, "bad data")).to_string(),
        "stream error"
    );
    assert_eq!(
        HarnessError::Aborted.to_string(),
        "operation aborted via cancellation token"
    );
}

/// Test 1.9 — `HarnessError` implements `std::error::Error`.
#[test]
fn implements_std_error() {
    let err = HarnessError::ModelThrottled;
    let _: &dyn Error = &err;
}

/// `NetworkError` and `StreamError` expose their source.
#[test]
fn source_chain() {
    let inner = io::Error::new(io::ErrorKind::ConnectionReset, "reset");
    let err = HarnessError::network(inner);
    assert!(err.source().is_some());

    let inner = io::Error::new(io::ErrorKind::InvalidData, "bad");
    let err = HarnessError::stream(inner);
    assert!(err.source().is_some());
}

/// `is_retryable` returns true only for the expected variants.
#[test]
fn retryable_classification() {
    assert!(HarnessError::ModelThrottled.is_retryable());
    assert!(HarnessError::network(io::Error::other("timeout")).is_retryable());

    assert!(!HarnessError::Aborted.is_retryable());
    assert!(!HarnessError::AlreadyRunning.is_retryable());
    assert!(!HarnessError::NoMessages.is_retryable());
    assert!(!HarnessError::InvalidContinue.is_retryable());
    assert!(!HarnessError::context_overflow("x").is_retryable());
    assert!(!HarnessError::structured_output_failed(1, "e").is_retryable());
    assert!(!HarnessError::stream(io::Error::other("x")).is_retryable());
}

/// Compile-time assertion: `HarnessError` is `Send + Sync`.
#[test]
fn send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<HarnessError>();
}
