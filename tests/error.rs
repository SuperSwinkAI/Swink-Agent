use std::error::Error;
use std::io;

use swink_agent::{AgentError, DowncastError};

/// Test 1.8 — `AgentError` variants display meaningful messages.
#[test]
fn display_messages() {
    assert_eq!(
        AgentError::context_overflow("claude-sonnet-4-6").to_string(),
        "context window overflow for model: claude-sonnet-4-6"
    );
    assert_eq!(
        AgentError::ModelThrottled.to_string(),
        "model request throttled (rate limited)"
    );
    assert_eq!(
        AgentError::network(io::Error::new(io::ErrorKind::ConnectionReset, "reset")).to_string(),
        "network error"
    );
    assert_eq!(
        AgentError::structured_output_failed(3, "schema mismatch").to_string(),
        "structured output failed after 3 attempts: schema mismatch"
    );
    assert_eq!(
        AgentError::AlreadyRunning.to_string(),
        "agent is already running"
    );
    assert_eq!(
        AgentError::NoMessages.to_string(),
        "cannot continue with empty message history"
    );
    assert_eq!(
        AgentError::InvalidContinue.to_string(),
        "cannot continue when last message is an assistant message"
    );
    assert_eq!(
        AgentError::stream(io::Error::new(io::ErrorKind::InvalidData, "bad data")).to_string(),
        "stream error"
    );
    assert_eq!(
        AgentError::Aborted.to_string(),
        "operation aborted via cancellation token"
    );
}

/// Test 1.9 — `AgentError` implements `std::error::Error`.
#[test]
fn implements_std_error() {
    let err = AgentError::ModelThrottled;
    let _: &dyn Error = &err;
}

/// `NetworkError` and `StreamError` expose their source.
#[test]
fn source_chain() {
    let inner = io::Error::new(io::ErrorKind::ConnectionReset, "reset");
    let err = AgentError::network(inner);
    assert!(err.source().is_some());

    let inner = io::Error::new(io::ErrorKind::InvalidData, "bad");
    let err = AgentError::stream(inner);
    assert!(err.source().is_some());
}

/// `is_retryable` returns true only for the expected variants.
#[test]
fn retryable_classification() {
    assert!(AgentError::ModelThrottled.is_retryable());
    assert!(AgentError::network(io::Error::other("timeout")).is_retryable());

    assert!(!AgentError::Aborted.is_retryable());
    assert!(!AgentError::AlreadyRunning.is_retryable());
    assert!(!AgentError::CacheMiss.is_retryable());
    assert!(!AgentError::NoMessages.is_retryable());
    assert!(!AgentError::InvalidContinue.is_retryable());
    assert!(!AgentError::context_overflow("x").is_retryable());
    assert!(!AgentError::structured_output_failed(1, "e").is_retryable());
    assert!(!AgentError::stream(io::Error::other("x")).is_retryable());
}

/// Compile-time assertion: `AgentError` is `Send + Sync`.
#[test]
fn send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AgentError>();
}

// ── New edge case tests ─────────────────────────────────────────────────────

/// Construct every `AgentError` variant and verify each is `Send + Sync`
/// at runtime (complements the compile-time assertion with concrete values).
#[test]
fn all_variants_are_send_sync() {
    fn assert_send_sync(_: &(impl Send + Sync)) {}

    let variants: Vec<AgentError> = vec![
        AgentError::context_overflow("model-x"),
        AgentError::ModelThrottled,
        AgentError::network(io::Error::other("net")),
        AgentError::structured_output_failed(1, "fail"),
        AgentError::AlreadyRunning,
        AgentError::NoMessages,
        AgentError::InvalidContinue,
        AgentError::stream(io::Error::other("stream")),
        AgentError::Aborted,
    ];

    for v in &variants {
        assert_send_sync(v);
    }
}

/// Every variant should produce a non-empty display string.
#[test]
fn display_all_variants_non_empty() {
    let variants: Vec<AgentError> = vec![
        AgentError::context_overflow("model-x"),
        AgentError::ModelThrottled,
        AgentError::network(io::Error::other("net")),
        AgentError::structured_output_failed(1, "fail"),
        AgentError::AlreadyRunning,
        AgentError::NoMessages,
        AgentError::InvalidContinue,
        AgentError::stream(io::Error::other("stream")),
        AgentError::Aborted,
    ];

    for v in &variants {
        let display = v.to_string();
        assert!(
            !display.is_empty(),
            "variant {v:?} has empty display string"
        );
    }
}

/// Wrap an `io::Error` in `NetworkError` and verify the `.source()` chain
/// points back to the original error.
#[test]
fn network_error_preserves_source() {
    let inner = io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused");
    let err = AgentError::network(inner);

    let source = err.source().expect("NetworkError should have a source");
    // The source's display should match the original io error message.
    assert_eq!(source.to_string(), "connection refused");
}

/// `Aborted` must not be retryable — cancellation is intentional.
#[test]
fn aborted_is_not_retryable() {
    assert!(
        !AgentError::Aborted.is_retryable(),
        "Aborted should not be retryable"
    );
}

/// `ContextWindowOverflow` is not retryable per the code — only `ModelThrottled`
/// and `NetworkError` are retryable. Context overflow is handled by the loop
/// via the `CONTEXT_OVERFLOW_SENTINEL` mechanism, not by the retry strategy.
#[test]
fn context_overflow_is_not_retryable() {
    let err = AgentError::context_overflow("claude-sonnet-4-6");
    assert!(
        !err.is_retryable(),
        "ContextWindowOverflow should not be retryable"
    );
}

/// Named constructors produce the correct variant discriminant.
#[test]
fn named_constructors_produce_correct_variants() {
    // AgentError::network()
    let err = AgentError::network(io::Error::other("test"));
    assert!(matches!(err, AgentError::NetworkError { .. }));

    // AgentError::stream()
    let err = AgentError::stream(io::Error::other("test"));
    assert!(matches!(err, AgentError::StreamError { .. }));

    // AgentError::context_overflow()
    let err = AgentError::context_overflow("model-y");
    assert!(matches!(
        err,
        AgentError::ContextWindowOverflow { ref model } if model == "model-y"
    ));

    // AgentError::structured_output_failed()
    let err = AgentError::structured_output_failed(5, "bad schema");
    assert!(matches!(
        err,
        AgentError::StructuredOutputFailed { attempts: 5, ref last_error }
            if last_error == "bad schema"
    ));
}

// ── T034: ContextWindowOverflow display contains model name ──

#[test]
fn error_context_overflow_display() {
    let err = AgentError::context_overflow("claude-sonnet-4-6");
    let display = err.to_string();
    assert!(
        display.contains("claude-sonnet-4-6"),
        "display should contain model name, got: {display}"
    );
}

// ── T035: StructuredOutputFailed display contains attempts and last_error ──

#[test]
fn error_structured_output_display() {
    let err = AgentError::structured_output_failed(3, "schema mismatch");
    let display = err.to_string();
    assert!(
        display.contains('3'),
        "display should contain attempt count, got: {display}"
    );
    assert!(
        display.contains("schema mismatch"),
        "display should contain last_error, got: {display}"
    );
}

// ── T036: All variants implement std::error::Error ──

#[test]
fn error_all_variants_implement_std_error() {
    let variants: Vec<AgentError> = vec![
        AgentError::context_overflow("model-x"),
        AgentError::ModelThrottled,
        AgentError::network(io::Error::other("net")),
        AgentError::structured_output_failed(1, "fail"),
        AgentError::AlreadyRunning,
        AgentError::NoMessages,
        AgentError::InvalidContinue,
        AgentError::stream(io::Error::other("stream")),
        AgentError::Aborted,
    ];

    for v in &variants {
        let _: &dyn Error = v;
    }
}

// ── T037: is_retryable classification ──

#[test]
fn error_retryable_classification() {
    assert!(AgentError::ModelThrottled.is_retryable());
    assert!(AgentError::network(io::Error::other("x")).is_retryable());

    assert!(!AgentError::CacheMiss.is_retryable());
    assert!(!AgentError::context_overflow("m").is_retryable());
    assert!(!AgentError::structured_output_failed(1, "e").is_retryable());
    assert!(!AgentError::AlreadyRunning.is_retryable());
    assert!(!AgentError::NoMessages.is_retryable());
    assert!(!AgentError::InvalidContinue.is_retryable());
    assert!(!AgentError::stream(io::Error::other("x")).is_retryable());
    assert!(!AgentError::Aborted.is_retryable());
}

// ── T038: StreamError source chain ──

#[test]
fn error_stream_error_source_chain() {
    let inner = io::Error::new(io::ErrorKind::InvalidData, "bad data");
    let err = AgentError::stream(inner);
    let source = err.source().expect("StreamError should have a source");
    assert_eq!(source.to_string(), "bad data");
}

// ── T039: DowncastError display ──

#[test]
fn downcast_error_display() {
    let err = DowncastError {
        expected: "MyCustomType",
        actual: "OtherType".into(),
    };
    let display = err.to_string();
    assert!(
        display.contains("MyCustomType"),
        "should contain expected type name, got: {display}"
    );
    assert!(
        display.contains("OtherType"),
        "should contain actual type name, got: {display}"
    );
    assert!(
        display.contains("Downcast failed"),
        "should contain 'Downcast failed', got: {display}"
    );

    // DowncastError implements std::error::Error
    let _: &dyn Error = &err;
}
