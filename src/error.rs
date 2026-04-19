//! Error types for the swink agent.
//!
//! All error conditions surfaced to the caller are represented as variants of
//! [`AgentError`]. Transient failures (`ModelThrottled`, `NetworkError`) are
//! retryable by the default strategy; all other variants are terminal for the
//! current operation unless a custom retry strategy opts into retrying them.

/// Error returned when downcasting an [`AgentMessage`](crate::types::AgentMessage) to a concrete
/// custom message type fails.
#[derive(Debug)]
pub struct DowncastError {
    /// The expected (target) type name.
    pub expected: &'static str,
    /// The actual type description found.
    pub actual: String,
}

impl std::fmt::Display for DowncastError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Downcast failed: expected {}, got {}",
            self.expected, self.actual
        )
    }
}

impl std::error::Error for DowncastError {}

/// The top-level error type for the swink agent.
///
/// Each variant maps to a specific failure mode described in PRD section 10.3.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
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

    /// An error from a plugin or extension.
    #[error("plugin error ({name})")]
    Plugin {
        name: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Provider-side context cache was not found (evicted or expired).
    ///
    /// The framework resets [`CacheState`](crate::context_cache::CacheState)
    /// before consulting the configured retry strategy. Custom strategies can
    /// choose to retry with `CacheHint::Write`.
    #[error("provider cache miss")]
    CacheMiss,

    /// Provider safety / content filter blocked the response.
    ///
    /// Non-retryable — the input triggered a provider-side content policy.
    /// Callers can match on this variant to distinguish safety blocks from
    /// auth or network errors.
    #[error("content filtered by provider safety policy")]
    ContentFiltered,

    /// A synchronous API (`prompt_sync`, `continue_sync`, etc.) was called
    /// from within an active Tokio runtime.
    ///
    /// These methods create their own Tokio runtime internally.  Calling them
    /// from async code (or any thread that already has a Tokio runtime) would
    /// panic.  Use the `_async` or `_stream` variants instead.
    #[error("sync API called inside an active Tokio runtime — use the async variant instead")]
    SyncInAsyncContext,

    /// The internal Tokio runtime used by blocking sync APIs failed to start.
    #[error("failed to create Tokio runtime for sync API")]
    RuntimeInit {
        #[source]
        source: std::io::Error,
    },
}

impl AgentError {
    /// Returns `true` for error variants that are safe to retry by default
    /// (`ModelThrottled` and `NetworkError`).
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::ModelThrottled | Self::NetworkError { .. })
    }

    /// Convenience constructor for [`AgentError::NetworkError`].
    pub fn network(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::NetworkError {
            source: Box::new(err),
        }
    }

    /// Convenience constructor for [`AgentError::StreamError`].
    pub fn stream(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::StreamError {
            source: Box::new(err),
        }
    }

    /// Convenience constructor for [`AgentError::ContextWindowOverflow`].
    pub fn context_overflow(model: impl Into<String>) -> Self {
        Self::ContextWindowOverflow {
            model: model.into(),
        }
    }

    /// Convenience constructor for [`AgentError::StructuredOutputFailed`].
    pub fn structured_output_failed(attempts: usize, last_error: impl Into<String>) -> Self {
        Self::StructuredOutputFailed {
            attempts,
            last_error: last_error.into(),
        }
    }

    /// Convenience constructor for [`AgentError::Plugin`].
    pub fn plugin(
        name: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Plugin {
            name: name.into(),
            source: Box::new(source),
        }
    }

    /// Convenience constructor for [`AgentError::RuntimeInit`].
    pub const fn runtime_init(source: std::io::Error) -> Self {
        Self::RuntimeInit { source }
    }
}

impl From<std::io::Error> for AgentError {
    fn from(err: std::io::Error) -> Self {
        Self::NetworkError {
            source: Box::new(err),
        }
    }
}

#[cfg(test)]
mod tests {
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
}
