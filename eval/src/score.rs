//! Scoring primitives for evaluation results.

use serde::{Deserialize, Serialize};

/// A numeric score in `[0.0, 1.0]` with a configurable pass threshold.
///
/// Each evaluator produces a `Score` for its metric. The threshold is
/// evaluator-specific, allowing different metrics to have different
/// pass/fail criteria.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Score {
    /// The numeric score, clamped to `[0.0, 1.0]`.
    pub value: f64,
    /// The minimum value required to pass. Defaults to `0.5`.
    pub threshold: f64,
}

impl Score {
    /// Create a new score with the given value and threshold.
    ///
    /// Values are clamped to `[0.0, 1.0]`.
    #[must_use]
    pub const fn new(value: f64, threshold: f64) -> Self {
        Self {
            value: value.clamp(0.0, 1.0),
            threshold: threshold.clamp(0.0, 1.0),
        }
    }

    /// A perfect passing score.
    #[must_use]
    pub const fn pass() -> Self {
        Self {
            value: 1.0,
            threshold: 0.5,
        }
    }

    /// A zero failing score.
    #[must_use]
    pub const fn fail() -> Self {
        Self {
            value: 0.0,
            threshold: 0.5,
        }
    }

    /// Derive the verdict from the score and threshold.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        if self.value >= self.threshold {
            Verdict::Pass
        } else {
            Verdict::Fail
        }
    }
}

impl Default for Score {
    fn default() -> Self {
        Self {
            value: 0.0,
            threshold: 0.5,
        }
    }
}

/// Binary pass/fail outcome derived from a [`Score`].
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail,
}

impl Verdict {
    /// Returns `true` if the verdict is [`Verdict::Pass`].
    #[must_use]
    pub const fn is_pass(&self) -> bool {
        matches!(self, Self::Pass)
    }
}

#[cfg(test)]
#[path = "score_tests.rs"]
mod tests;
