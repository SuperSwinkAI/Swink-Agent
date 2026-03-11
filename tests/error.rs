use std::error::Error;
use std::io;

use agent_harness::HarnessError;

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

// ── New edge case tests ─────────────────────────────────────────────────────

/// Construct every `HarnessError` variant and verify each is `Send + Sync`
/// at runtime (complements the compile-time assertion with concrete values).
#[test]
fn all_variants_are_send_sync() {
    fn assert_send_sync(_: &(impl Send + Sync)) {}

    let variants: Vec<HarnessError> = vec![
        HarnessError::context_overflow("model-x"),
        HarnessError::ModelThrottled,
        HarnessError::network(io::Error::other("net")),
        HarnessError::structured_output_failed(1, "fail"),
        HarnessError::AlreadyRunning,
        HarnessError::NoMessages,
        HarnessError::InvalidContinue,
        HarnessError::stream(io::Error::other("stream")),
        HarnessError::Aborted,
    ];

    for v in &variants {
        assert_send_sync(v);
    }
}

/// Every variant should produce a non-empty display string.
#[test]
fn display_all_variants_non_empty() {
    let variants: Vec<HarnessError> = vec![
        HarnessError::context_overflow("model-x"),
        HarnessError::ModelThrottled,
        HarnessError::network(io::Error::other("net")),
        HarnessError::structured_output_failed(1, "fail"),
        HarnessError::AlreadyRunning,
        HarnessError::NoMessages,
        HarnessError::InvalidContinue,
        HarnessError::stream(io::Error::other("stream")),
        HarnessError::Aborted,
    ];

    for v in &variants {
        let display = v.to_string();
        assert!(!display.is_empty(), "variant {v:?} has empty display string");
    }
}

/// Wrap an `io::Error` in `NetworkError` and verify the `.source()` chain
/// points back to the original error.
#[test]
fn network_error_preserves_source() {
    let inner = io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused");
    let err = HarnessError::network(inner);

    let source = err.source().expect("NetworkError should have a source");
    // The source's display should match the original io error message.
    assert_eq!(source.to_string(), "connection refused");
}

/// `Aborted` must not be retryable — cancellation is intentional.
#[test]
fn aborted_is_not_retryable() {
    assert!(
        !HarnessError::Aborted.is_retryable(),
        "Aborted should not be retryable"
    );
}

/// `ContextWindowOverflow` is not retryable per the code — only `ModelThrottled`
/// and `NetworkError` are retryable. Context overflow is handled by the loop
/// via the `CONTEXT_OVERFLOW_SENTINEL` mechanism, not by the retry strategy.
#[test]
fn context_overflow_is_not_retryable() {
    let err = HarnessError::context_overflow("claude-sonnet-4-6");
    assert!(
        !err.is_retryable(),
        "ContextWindowOverflow should not be retryable"
    );
}

/// Named constructors produce the correct variant discriminant.
#[test]
fn named_constructors_produce_correct_variants() {
    // HarnessError::network()
    let err = HarnessError::network(io::Error::other("test"));
    assert!(matches!(err, HarnessError::NetworkError { .. }));

    // HarnessError::stream()
    let err = HarnessError::stream(io::Error::other("test"));
    assert!(matches!(err, HarnessError::StreamError { .. }));

    // HarnessError::context_overflow()
    let err = HarnessError::context_overflow("model-y");
    assert!(matches!(
        err,
        HarnessError::ContextWindowOverflow { ref model } if model == "model-y"
    ));

    // HarnessError::structured_output_failed()
    let err = HarnessError::structured_output_failed(5, "bad schema");
    assert!(matches!(
        err,
        HarnessError::StructuredOutputFailed { attempts: 5, ref last_error }
            if last_error == "bad schema"
    ));
}
