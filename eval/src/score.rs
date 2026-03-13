//! Scoring primitives for evaluation results.

use serde::{Deserialize, Serialize};

/// A numeric score in `[0.0, 1.0]` with a configurable pass threshold.
///
/// Each evaluator produces a `Score` for its metric. The threshold is
/// evaluator-specific, allowing different metrics to have different
/// pass/fail criteria.
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
mod tests {
    use super::*;

    #[test]
    fn score_pass_verdict() {
        let s = Score::new(0.8, 0.5);
        assert_eq!(s.verdict(), Verdict::Pass);
    }

    #[test]
    fn score_fail_verdict() {
        let s = Score::new(0.3, 0.5);
        assert_eq!(s.verdict(), Verdict::Fail);
    }

    #[test]
    fn score_at_threshold_passes() {
        let s = Score::new(0.5, 0.5);
        assert_eq!(s.verdict(), Verdict::Pass);
    }

    #[test]
    fn score_clamps_to_bounds() {
        let s = Score::new(1.5, -0.1);
        assert!((s.value - 1.0).abs() < f64::EPSILON);
        assert!((s.threshold - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn pass_and_fail_constructors() {
        assert_eq!(Score::pass().verdict(), Verdict::Pass);
        assert_eq!(Score::fail().verdict(), Verdict::Fail);
    }

    #[test]
    fn verdict_is_pass() {
        assert!(Verdict::Pass.is_pass());
        assert!(!Verdict::Fail.is_pass());
    }
}
