//! Mid-turn budget guard for pre-call cost and token gating.
//!
//! [`BudgetGuard`] is checked **before** each LLM call in the inner loop,
//! comparing accumulated cost and token usage against configured limits.
//! This complements [`CostCapPolicy`](crate::CostCapPolicy), which only
//! runs **after** a turn completes.

use crate::types::{Cost, Usage};

// ─── BudgetGuard ─────────────────────────────────────────────────────────────

/// Pre-call budget limits that prevent an LLM call from starting when
/// accumulated cost or token usage has already exceeded the budget.
///
/// Unlike [`CostCapPolicy`](crate::CostCapPolicy) (a [`LoopPolicy`](crate::LoopPolicy)
/// checked after each turn), `BudgetGuard` is evaluated **before** each LLM
/// call, providing tighter control over spend.
///
/// # Examples
///
/// ```
/// use swink_agent::BudgetGuard;
///
/// // Cost-only guard
/// let guard = BudgetGuard::new().with_max_cost(5.0);
///
/// // Token-only guard
/// let guard = BudgetGuard::new().with_max_tokens(100_000);
///
/// // Both limits
/// let guard = BudgetGuard::new()
///     .with_max_cost(5.0)
///     .with_max_tokens(100_000);
/// ```
#[derive(Debug, Clone)]
pub struct BudgetGuard {
    /// Maximum total cost before blocking further LLM calls.
    pub max_cost: Option<f64>,
    /// Maximum total tokens before blocking further LLM calls.
    pub max_tokens: Option<u64>,
}

impl BudgetGuard {
    /// Create a new `BudgetGuard` with no limits set.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_cost: None,
            max_tokens: None,
        }
    }

    /// Set the maximum total cost.
    #[must_use]
    pub const fn with_max_cost(mut self, max_cost: f64) -> Self {
        self.max_cost = Some(max_cost);
        self
    }

    /// Set the maximum total tokens.
    #[must_use]
    pub const fn with_max_tokens(mut self, max_tokens: u64) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Check whether accumulated usage and cost are within budget.
    ///
    /// Returns `Ok(())` if the budget has not been exceeded, or
    /// `Err(BudgetExceeded)` describing which limit was hit.
    pub const fn check(&self, usage: &Usage, cost: &Cost) -> Result<(), BudgetExceeded> {
        if let Some(max_cost) = self.max_cost
            && cost.total > max_cost
        {
            return Err(BudgetExceeded::Cost {
                limit: max_cost,
                actual: cost.total,
            });
        }
        if let Some(max_tokens) = self.max_tokens
            && usage.total > max_tokens
        {
            return Err(BudgetExceeded::Tokens {
                limit: max_tokens,
                actual: usage.total,
            });
        }
        Ok(())
    }
}

impl Default for BudgetGuard {
    fn default() -> Self {
        Self::new()
    }
}

// ─── BudgetExceeded ──────────────────────────────────────────────────────────

/// Describes which budget limit was exceeded.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetExceeded {
    /// Accumulated cost exceeded the configured maximum.
    Cost { limit: f64, actual: f64 },
    /// Accumulated token usage exceeded the configured maximum.
    Tokens { limit: u64, actual: u64 },
}

impl std::fmt::Display for BudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cost { limit, actual } => {
                write!(
                    f,
                    "budget exceeded: cost {actual:.4} exceeds limit {limit:.4}"
                )
            }
            Self::Tokens { limit, actual } => {
                write!(f, "budget exceeded: {actual} tokens exceeds limit {limit}")
            }
        }
    }
}

// ─── Compile-time Send + Sync assertions ────────────────────────────────────

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<BudgetGuard>();
    assert_send_sync::<BudgetExceeded>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_limits_always_passes() {
        let guard = BudgetGuard::new();
        let usage = Usage {
            total: 999_999,
            ..Default::default()
        };
        let cost = Cost {
            total: 999.0,
            ..Default::default()
        };
        assert!(guard.check(&usage, &cost).is_ok());
    }

    #[test]
    fn cost_under_limit_passes() {
        let guard = BudgetGuard::new().with_max_cost(5.0);
        let usage = Usage::default();
        let cost = Cost {
            total: 3.0,
            ..Default::default()
        };
        assert!(guard.check(&usage, &cost).is_ok());
    }

    #[test]
    fn cost_over_limit_fails() {
        let guard = BudgetGuard::new().with_max_cost(5.0);
        let usage = Usage::default();
        let cost = Cost {
            total: 5.01,
            ..Default::default()
        };
        let err = guard.check(&usage, &cost).unwrap_err();
        assert!(
            matches!(err, BudgetExceeded::Cost { limit, actual } if limit == 5.0 && actual == 5.01)
        );
    }

    #[test]
    fn cost_at_limit_passes() {
        let guard = BudgetGuard::new().with_max_cost(5.0);
        let usage = Usage::default();
        let cost = Cost {
            total: 5.0,
            ..Default::default()
        };
        assert!(guard.check(&usage, &cost).is_ok());
    }

    #[test]
    fn tokens_under_limit_passes() {
        let guard = BudgetGuard::new().with_max_tokens(100_000);
        let usage = Usage {
            total: 50_000,
            ..Default::default()
        };
        let cost = Cost::default();
        assert!(guard.check(&usage, &cost).is_ok());
    }

    #[test]
    fn tokens_over_limit_fails() {
        let guard = BudgetGuard::new().with_max_tokens(100_000);
        let usage = Usage {
            total: 100_001,
            ..Default::default()
        };
        let cost = Cost::default();
        let err = guard.check(&usage, &cost).unwrap_err();
        assert!(
            matches!(err, BudgetExceeded::Tokens { limit, actual } if limit == 100_000 && actual == 100_001)
        );
    }

    #[test]
    fn tokens_at_limit_passes() {
        let guard = BudgetGuard::new().with_max_tokens(100_000);
        let usage = Usage {
            total: 100_000,
            ..Default::default()
        };
        let cost = Cost::default();
        assert!(guard.check(&usage, &cost).is_ok());
    }

    #[test]
    fn both_limits_cost_exceeds_first() {
        let guard = BudgetGuard::new()
            .with_max_cost(1.0)
            .with_max_tokens(100_000);
        let usage = Usage {
            total: 50_000,
            ..Default::default()
        };
        let cost = Cost {
            total: 1.5,
            ..Default::default()
        };
        let err = guard.check(&usage, &cost).unwrap_err();
        assert!(matches!(err, BudgetExceeded::Cost { .. }));
    }

    #[test]
    fn both_limits_tokens_exceeds_when_cost_ok() {
        let guard = BudgetGuard::new().with_max_cost(10.0).with_max_tokens(100);
        let usage = Usage {
            total: 200,
            ..Default::default()
        };
        let cost = Cost {
            total: 1.0,
            ..Default::default()
        };
        let err = guard.check(&usage, &cost).unwrap_err();
        assert!(matches!(err, BudgetExceeded::Tokens { .. }));
    }

    #[test]
    fn both_limits_both_ok() {
        let guard = BudgetGuard::new()
            .with_max_cost(10.0)
            .with_max_tokens(100_000);
        let usage = Usage {
            total: 50_000,
            ..Default::default()
        };
        let cost = Cost {
            total: 5.0,
            ..Default::default()
        };
        assert!(guard.check(&usage, &cost).is_ok());
    }

    #[test]
    fn display_cost_exceeded() {
        let err = BudgetExceeded::Cost {
            limit: 5.0,
            actual: 5.5,
        };
        assert!(err.to_string().contains("cost"));
        assert!(err.to_string().contains("5.5"));
    }

    #[test]
    fn display_tokens_exceeded() {
        let err = BudgetExceeded::Tokens {
            limit: 100,
            actual: 200,
        };
        assert!(err.to_string().contains("200 tokens"));
    }

    #[test]
    fn default_is_no_limits() {
        let guard = BudgetGuard::default();
        assert!(guard.max_cost.is_none());
        assert!(guard.max_tokens.is_none());
    }
}
