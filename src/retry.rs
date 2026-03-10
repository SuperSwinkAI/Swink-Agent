//! Retry strategy trait and default exponential back-off implementation.
//!
//! The [`RetryStrategy`] trait defines the contract for deciding whether a
//! failed model call should be retried and how long to wait before the next
//! attempt. [`DefaultRetryStrategy`] provides exponential back-off with
//! optional jitter, a configurable attempt cap, and a maximum delay ceiling.

use std::time::Duration;

use crate::error::HarnessError;

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
    fn should_retry(&self, error: &HarnessError, attempt: u32) -> bool;

    /// Returns the duration to wait before attempt number `attempt`.
    /// Attempt numbering starts at 1.
    fn delay(&self, attempt: u32) -> Duration;
}

// ---------------------------------------------------------------------------
// Default implementation
// ---------------------------------------------------------------------------

/// Exponential back-off retry strategy with optional jitter.
///
/// Only transient errors ([`HarnessError::ModelThrottled`] and
/// [`HarnessError::NetworkError`]) are retried. All other error variants are
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
    fn should_retry(&self, error: &HarnessError, attempt: u32) -> bool {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    // -- Send + Sync compile-time assertions --------------------------------

    #[test]
    fn trait_object_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DefaultRetryStrategy>();
        assert_send_sync::<Box<dyn RetryStrategy>>();
    }

    // -- 2.7: Retries ModelThrottled up to max_attempts ---------------------

    #[test]
    fn retries_model_throttled_up_to_max_attempts() {
        let strategy = DefaultRetryStrategy::default().with_max_attempts(3);

        // Attempts 1 and 2 should retry.
        assert!(strategy.should_retry(&HarnessError::ModelThrottled, 1));
        assert!(strategy.should_retry(&HarnessError::ModelThrottled, 2));

        // Attempt 3 (== max_attempts) should NOT retry.
        assert!(!strategy.should_retry(&HarnessError::ModelThrottled, 3));
    }

    #[test]
    fn retries_network_error_up_to_max_attempts() {
        let strategy = DefaultRetryStrategy::default().with_max_attempts(3);
        let err = HarnessError::network(io::Error::other("timeout"));

        assert!(strategy.should_retry(&err, 1));
        assert!(strategy.should_retry(&err, 2));
        assert!(!strategy.should_retry(&err, 3));
    }

    // -- 2.8: Does not retry ContextWindowOverflow --------------------------

    #[test]
    fn does_not_retry_context_window_overflow() {
        let strategy = DefaultRetryStrategy::default();
        let err = HarnessError::ContextWindowOverflow {
            model: "test-model".into(),
        };

        assert!(!strategy.should_retry(&err, 1));
    }

    #[test]
    fn does_not_retry_non_retryable_variants() {
        let strategy = DefaultRetryStrategy::default();

        assert!(!strategy.should_retry(&HarnessError::Aborted, 1));
        assert!(!strategy.should_retry(&HarnessError::AlreadyRunning, 1));
        assert!(!strategy.should_retry(&HarnessError::NoMessages, 1));
        assert!(!strategy.should_retry(&HarnessError::InvalidContinue, 1));
        assert!(!strategy.should_retry(
            &HarnessError::StructuredOutputFailed {
                attempts: 3,
                last_error: "bad".into(),
            },
            1
        ));
        assert!(!strategy.should_retry(&HarnessError::stream(io::Error::other("bad data")), 1));
    }

    // -- 2.9: Delay increases exponentially and caps at max_delay -----------

    #[test]
    fn delay_increases_exponentially_without_jitter() {
        let strategy = DefaultRetryStrategy::default()
            .with_base_delay(Duration::from_secs(1))
            .with_multiplier(2.0)
            .with_jitter(false);

        assert_eq!(strategy.delay(1), Duration::from_secs(1)); // 1 * 2^0 = 1
        assert_eq!(strategy.delay(2), Duration::from_secs(2)); // 1 * 2^1 = 2
        assert_eq!(strategy.delay(3), Duration::from_secs(4)); // 1 * 2^2 = 4
    }

    #[test]
    fn delay_caps_at_max_delay() {
        let strategy = DefaultRetryStrategy::default()
            .with_base_delay(Duration::from_secs(1))
            .with_multiplier(2.0)
            .with_max_delay(Duration::from_secs(3))
            .with_jitter(false);

        assert_eq!(strategy.delay(1), Duration::from_secs(1));
        assert_eq!(strategy.delay(2), Duration::from_secs(2));
        // 1 * 2^2 = 4, but capped at 3.
        assert_eq!(strategy.delay(3), Duration::from_secs(3));
    }

    // -- 2.10: Jitter produces varying delays -------------------------------

    #[test]
    fn jitter_produces_varying_delays() {
        let strategy = DefaultRetryStrategy::default()
            .with_base_delay(Duration::from_secs(10))
            .with_jitter(true);

        let delays: Vec<Duration> = (0..20).map(|_| strategy.delay(2)).collect();

        // With jitter over 20 samples on a 10s base delay, it is
        // astronomically unlikely that every value is identical.
        let all_same = delays.windows(2).all(|w| w[0] == w[1]);
        assert!(
            !all_same,
            "expected varying delays with jitter enabled, but all 20 samples were identical"
        );
    }

    // -- Builder methods ----------------------------------------------------

    #[test]
    fn builder_methods() {
        let strategy = DefaultRetryStrategy::default()
            .with_max_attempts(5)
            .with_base_delay(Duration::from_millis(500))
            .with_max_delay(Duration::from_secs(30))
            .with_multiplier(3.0)
            .with_jitter(false);

        assert_eq!(strategy.max_attempts, 5);
        assert_eq!(strategy.base_delay, Duration::from_millis(500));
        assert_eq!(strategy.max_delay, Duration::from_secs(30));
        assert!((strategy.multiplier - 3.0).abs() < f64::EPSILON);
        assert!(!strategy.jitter);
    }
}
