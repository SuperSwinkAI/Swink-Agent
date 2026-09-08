//! Tests for `mod`.
#![cfg(test)]

use super::*;

#[test]
fn classify_cache_miss_variants() {
    let cases = [
        "cache miss",
        "Cache Miss detected",
        "provider cache_miss",
        "cache not found",
    ];
    for msg in cases {
        let err = classify_stream_error(msg, StopReason::Error, None);
        assert!(
            matches!(err, AgentError::CacheMiss),
            "expected CacheMiss for \"{msg}\", got {err:?}"
        );
        assert!(!err.is_retryable());
    }
}

#[test]
fn classify_non_cache_miss() {
    let err = classify_stream_error("internal server error", StopReason::Error, None);
    assert!(!matches!(err, AgentError::CacheMiss));
}

#[test]
fn classify_content_filtered_by_kind() {
    let err = classify_stream_error(
        "response blocked",
        StopReason::Error,
        Some(StreamErrorKind::ContentFiltered),
    );
    assert!(matches!(err, AgentError::ContentFiltered));
    assert!(!err.is_retryable());
}

#[test]
fn classify_content_filtered_by_string() {
    let err = classify_stream_error("content filter violation detected", StopReason::Error, None);
    assert!(matches!(err, AgentError::ContentFiltered));
    assert!(!err.is_retryable());
}

#[test]
fn classify_throttled_by_kind() {
    let err = classify_stream_error(
        "some error",
        StopReason::Error,
        Some(StreamErrorKind::Throttled),
    );
    assert!(matches!(err, AgentError::ModelThrottled));
}

#[test]
fn classify_network_by_kind() {
    let err = classify_stream_error(
        "connection reset",
        StopReason::Error,
        Some(StreamErrorKind::Network),
    );
    assert!(matches!(err, AgentError::NetworkError { .. }));
    assert!(err.is_retryable());
}

#[test]
fn classify_auth_by_kind() {
    let err = classify_stream_error(
        "invalid api key",
        StopReason::Error,
        Some(StreamErrorKind::Auth),
    );
    match err {
        AgentError::Auth { ref message } => assert_eq!(message, "invalid api key"),
        ref other => panic!("expected Auth, got {other:?}"),
    }
    assert!(!err.is_retryable());
}

#[test]
fn classify_auth_by_string() {
    for msg in [
        "Unauthorized",
        "invalid API key provided",
        "authentication failed for tenant",
        "403 Forbidden",
    ] {
        let err = classify_stream_error(msg, StopReason::Error, None);
        assert!(
            matches!(err, AgentError::Auth { .. }),
            "expected Auth for \"{msg}\", got {err:?}"
        );
    }
    // Bare 3-digit codes must not trigger auth classification: token
    // counts and ids contain "401"/"403" too easily.
    let err = classify_stream_error(
        "request used 40123 tokens of budget",
        StopReason::Error,
        None,
    );
    assert!(!matches!(err, AgentError::Auth { .. }));
}

#[test]
fn classify_for_model_names_model_in_overflow() {
    // Structural path
    let err = classify_stream_error_for_model(
        "claude-fable-5",
        "too many tokens",
        StopReason::Error,
        Some(StreamErrorKind::ContextWindowExceeded),
    );
    match err {
        AgentError::ContextWindowOverflow { ref model } => assert_eq!(model, "claude-fable-5"),
        ref other => panic!("expected ContextWindowOverflow, got {other:?}"),
    }
    // String-matching fallback path
    let err = classify_stream_error_for_model(
        "claude-fable-5",
        "context_length_exceeded: too long",
        StopReason::Error,
        None,
    );
    match err {
        AgentError::ContextWindowOverflow { ref model } => assert_eq!(model, "claude-fable-5"),
        ref other => panic!("expected ContextWindowOverflow, got {other:?}"),
    }
}

#[test]
fn classify_context_overflow_by_kind() {
    let err = classify_stream_error(
        "too many tokens",
        StopReason::Error,
        Some(StreamErrorKind::ContextWindowExceeded),
    );
    assert!(matches!(err, AgentError::ContextWindowOverflow { .. }));
}

#[test]
fn classify_model_retired_by_kind() {
    let err = classify_stream_error(
        "OpenAI model retired or unavailable (HTTP 404): model_not_found",
        StopReason::Error,
        Some(StreamErrorKind::ModelRetired),
    );
    match err {
        AgentError::ModelRetired { ref message } => {
            assert!(message.contains("model_not_found"));
        }
        ref other => panic!("expected ModelRetired, got {other:?}"),
    }
    assert!(!err.is_retryable());
}

#[test]
fn classify_model_retired_by_string() {
    for msg in [
        "error code model_not_found for gpt-4-32k",
        "the model claude-1 has been decommissioned",
        "model gemini-1.0 has been retired",
    ] {
        let err = classify_stream_error(msg, StopReason::Error, None);
        assert!(
            matches!(err, AgentError::ModelRetired { .. }),
            "expected ModelRetired for \"{msg}\", got {err:?}"
        );
    }
    // Plain mention of "model" must not trigger retirement.
    let err = classify_stream_error("model produced invalid output", StopReason::Error, None);
    assert!(!matches!(err, AgentError::ModelRetired { .. }));
}

#[test]
fn structured_kind_takes_priority_over_string() {
    // Message says "rate limit" but kind says Network — kind wins
    let err = classify_stream_error(
        "rate limit exceeded",
        StopReason::Error,
        Some(StreamErrorKind::Network),
    );
    assert!(
        matches!(err, AgentError::NetworkError { .. }),
        "structured kind should override string matching, got {err:?}"
    );
}

#[test]
fn string_fallback_for_unclassified_errors() {
    // No error_kind — string matching should still work for external adapters
    let err = classify_stream_error("rate limit (429)", StopReason::Error, None);
    assert!(matches!(err, AgentError::ModelThrottled));
}

#[test]
fn string_fallback_context_overflow() {
    let err = classify_stream_error("context_length_exceeded: too long", StopReason::Error, None);
    assert!(matches!(err, AgentError::ContextWindowOverflow { .. }));
}

#[test]
fn aborted_stop_reason_without_kind() {
    let err = classify_stream_error("operation cancelled", StopReason::Aborted, None);
    assert!(matches!(err, AgentError::Aborted));
}
