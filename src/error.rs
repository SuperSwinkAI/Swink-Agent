//! Error types for the agent harness.
//!
//! All error conditions surfaced to the caller are represented as variants of
//! [`HarnessError`]. Transient failures (`ModelThrottled`, `NetworkError`) are
//! retryable; all other variants are terminal for the current operation.

/// The top-level error type for the agent harness.
///
/// Each variant maps to a specific failure mode described in PRD section 10.3.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    /// Provider rejected the request because input exceeds the model's context window.
    #[error("context window overflow for model: {model}")]
    ContextWindowOverflow { model: String },

    /// Rate limit / 429 received from the provider.
    #[error("model request throttled (rate limited)")]
    ModelThrottled,

    /// Transient IO or connection failure.
    #[error("network error")]
    NetworkError {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Structured output validation failed after exhausting all retry attempts.
    #[error("structured output failed after {attempts} attempts: {last_error}")]
    StructuredOutputFailed { attempts: usize, last_error: String },

    /// `prompt()` was called while a run is already active.
    #[error("agent is already running")]
    AlreadyRunning,

    /// `continue_loop()` was called with an empty message history.
    #[error("cannot continue with empty message history")]
    NoMessages,

    /// `continue_loop()` was called when the last message is an assistant message.
    #[error("cannot continue when last message is an assistant message")]
    InvalidContinue,

    /// Non-retryable failure from the `StreamFn` implementation.
    #[error("stream error")]
    StreamError {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The operation was cancelled via a `CancellationToken`.
    #[error("operation aborted via cancellation token")]
    Aborted,
}

impl HarnessError {
    /// Returns `true` for error variants that are safe to retry
    /// (`ModelThrottled` and `NetworkError`).
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::ModelThrottled | Self::NetworkError { .. })
    }

    /// Convenience constructor for [`HarnessError::NetworkError`].
    pub fn network(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::NetworkError {
            source: Box::new(err),
        }
    }

    /// Convenience constructor for [`HarnessError::StreamError`].
    pub fn stream(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::StreamError {
            source: Box::new(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;
    use std::io;

    /// Test 1.8 — `HarnessError` variants display meaningful messages.
    #[test]
    fn display_messages() {
        assert_eq!(
            HarnessError::ContextWindowOverflow {
                model: "claude-sonnet-4-6".into()
            }
            .to_string(),
            "context window overflow for model: claude-sonnet-4-6"
        );
        assert_eq!(
            HarnessError::ModelThrottled.to_string(),
            "model request throttled (rate limited)"
        );
        assert_eq!(
            HarnessError::network(io::Error::new(io::ErrorKind::ConnectionReset, "reset"))
                .to_string(),
            "network error"
        );
        assert_eq!(
            HarnessError::StructuredOutputFailed {
                attempts: 3,
                last_error: "schema mismatch".into()
            }
            .to_string(),
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
            HarnessError::stream(io::Error::new(io::ErrorKind::InvalidData, "bad data"))
                .to_string(),
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
        // Coerce to trait object to prove Error is implemented.
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
        assert!(!HarnessError::ContextWindowOverflow { model: "x".into() }.is_retryable());
        assert!(
            !HarnessError::StructuredOutputFailed {
                attempts: 1,
                last_error: "e".into()
            }
            .is_retryable()
        );
        assert!(!HarnessError::stream(io::Error::other("x")).is_retryable());
    }

    /// Compile-time assertion: `HarnessError` is `Send + Sync`.
    #[test]
    fn send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<HarnessError>();
    }
}
