//! Retry strategy trait and default exponential back-off implementation.
//!
//! The [`RetryStrategy`] trait defines the contract for deciding whether a
//! failed model call should be retried and how long to wait before the next
//! attempt. [`DefaultRetryStrategy`] provides exponential back-off with
//! optional jitter, a configurable attempt cap, and a maximum delay ceiling.

use std::time::Duration;

use crate::error::AgentError;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Determines whether a failed model call should be retried and, if so, how
/// long to wait before the next attempt.
///
/// Implementations must be object-safe (`Send + Sync`) so that the strategy
/// can be stored as `Box<dyn RetryStrategy>` inside loop configuration.
pub trait RetryStrategy: Send + Sync {
    /// Returns `true` if `error` on the given `attempt` number should be
    /// retried. Attempt numbering starts at 1.
    fn should_retry(&self, error: &AgentError, attempt: u32) -> bool;

    /// Returns the duration to wait before attempt number `attempt`.
    /// Attempt numbering starts at 1.
    fn delay(&self, attempt: u32) -> Duration;
}

// ---------------------------------------------------------------------------
// Default implementation
// ---------------------------------------------------------------------------

/// Exponential back-off retry strategy with optional jitter.
///
/// Only transient errors ([`AgentError::ModelThrottled`] and
/// [`AgentError::NetworkError`]) are retried. All other error variants are
/// considered non-retryable and cause an immediate exit.
///
/// # Defaults
///
/// | Field | Default |
/// |---|---|
/// | `max_attempts` | 3 |
/// | `base_delay` | 1 second |
/// | `max_delay` | 60 seconds |
/// | `multiplier` | 2.0 |
/// | `jitter` | `true` |
#[derive(Debug, Clone)]
pub struct DefaultRetryStrategy {
    /// Maximum number of attempts (including the first). The strategy returns
    /// `false` from `should_retry` once `attempt >= max_attempts`.
    pub max_attempts: u32,

    /// Base delay before the first retry (attempt 1).
    pub base_delay: Duration,

    /// Upper bound on the computed delay — the delay is capped at this value
    /// regardless of the exponential growth.
    pub max_delay: Duration,

    /// Multiplicative factor applied per attempt.
    pub multiplier: f64,

    /// When `true`, the computed delay is multiplied by a random factor in
    /// `[0.5, 1.5)` to spread out retries across concurrent callers.
    pub jitter: bool,
}

impl Default for DefaultRetryStrategy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            multiplier: 2.0,
            jitter: true,
        }
    }
}

impl DefaultRetryStrategy {
    /// Set the maximum number of attempts.
    #[must_use]
    pub const fn with_max_attempts(mut self, n: u32) -> Self {
        self.max_attempts = n;
        self
    }

    /// Set the base delay before the first retry.
    #[must_use]
    pub const fn with_base_delay(mut self, d: Duration) -> Self {
        self.base_delay = d;
        self
    }

    /// Set the maximum delay cap.
    #[must_use]
    pub const fn with_max_delay(mut self, d: Duration) -> Self {
        self.max_delay = d;
        self
    }

    /// Set the exponential multiplier.
    #[must_use]
    pub const fn with_multiplier(mut self, m: f64) -> Self {
        self.multiplier = m;
        self
    }

    /// Enable or disable jitter.
    #[must_use]
    pub const fn with_jitter(mut self, j: bool) -> Self {
        self.jitter = j;
        self
    }
}

impl RetryStrategy for DefaultRetryStrategy {
    fn should_retry(&self, error: &AgentError, attempt: u32) -> bool {
        if attempt >= self.max_attempts {
            return false;
        }
        error.is_retryable()
    }

    fn delay(&self, attempt: u32) -> Duration {
        // Exponential back-off: base_delay * multiplier^(attempt - 1)
        let exp = self
            .multiplier
            .powi(attempt.saturating_sub(1).try_into().unwrap_or(i32::MAX));
        let base_secs = self.base_delay.as_secs_f64() * exp;

        // Cap at max_delay.
        let capped_secs = base_secs.min(self.max_delay.as_secs_f64());

        // Optionally apply jitter: multiply by a random factor in [0.5, 1.5).
        let final_secs = if self.jitter {
            let jitter_factor = 0.5 + rand::random::<f64>(); // [0.5, 1.5)
            capped_secs * jitter_factor
        } else {
            capped_secs
        };

        Duration::from_secs_f64(final_secs)
    }
}
