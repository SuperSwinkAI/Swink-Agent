//! Shared retry and blocking helpers for judge clients.

use std::future::Future;
use std::time::Duration;

/// Shared retry policy scaffold for judge clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Maximum number of attempts, including the initial request.
    pub max_attempts: usize,
    /// Upper bound for backoff delay between attempts.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 6,
            max_delay: Duration::from_mins(4),
        }
    }
}

/// Returns the retry policy that downstream clients should apply.
#[must_use]
pub const fn build_retry(policy: RetryPolicy) -> RetryPolicy {
    policy
}

/// Extension trait for provider-specific blocking wrappers.
pub trait BlockingExt: Future + Send + 'static
where
    Self::Output: Send + 'static,
{
    /// Block on the future using the current Tokio runtime handle.
    fn block_on(self) -> Self::Output
    where
        Self: Sized,
    {
        tokio::runtime::Handle::current().block_on(self)
    }
}

impl<F> BlockingExt for F
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
}
