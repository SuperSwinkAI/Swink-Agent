//! Aggregation strategies for combining evaluator outputs.

use crate::score::Score;

/// Reduces multiple metric samples into a single composite score.
pub trait Aggregator: Send + Sync {
    /// Aggregate the provided metric samples into one score.
    ///
    /// Empty inputs return [`Score::default()`].
    fn aggregate(&self, samples: &[Score]) -> Score;
}

/// Default arithmetic-mean aggregator.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default)]
pub struct Average;

impl Average {
    /// Create a new `Average`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Aggregator for Average {
    fn aggregate(&self, samples: &[Score]) -> Score {
        mean_score(samples).unwrap_or_default()
    }
}

/// Passes only when every sample passes.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default)]
pub struct AllPass;

impl AllPass {
    /// Create a new `AllPass`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Aggregator for AllPass {
    fn aggregate(&self, samples: &[Score]) -> Score {
        if samples.is_empty() {
            return Score::default();
        }

        if samples.iter().all(|sample| sample.verdict().is_pass()) {
            Score::pass()
        } else {
            Score::fail()
        }
    }
}

/// Passes when any sample passes.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default)]
pub struct AnyPass;

impl AnyPass {
    /// Create a new `AnyPass`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Aggregator for AnyPass {
    fn aggregate(&self, samples: &[Score]) -> Score {
        if samples.is_empty() {
            return Score::default();
        }

        if samples.iter().any(|sample| sample.verdict().is_pass()) {
            Score::pass()
        } else {
            Score::fail()
        }
    }
}

/// Weighted arithmetic-mean aggregator.
///
/// When the configured weight count does not match the sample count, the
/// aggregator falls back to the unweighted mean rather than silently dropping
/// or over-reading samples.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct Weighted {
    pub weights: Vec<f64>,
}

impl Weighted {
    /// Create a weighted aggregator from the given per-sample weights.
    #[must_use]
    pub fn new(weights: Vec<f64>) -> Self {
        Self { weights }
    }
}

impl Aggregator for Weighted {
    fn aggregate(&self, samples: &[Score]) -> Score {
        if samples.is_empty() {
            return Score::default();
        }

        if self.weights.len() != samples.len() {
            return Average.aggregate(samples);
        }

        let total_weight: f64 = self.weights.iter().copied().filter(|w| *w > 0.0).sum();
        if total_weight <= 0.0 {
            return Average.aggregate(samples);
        }

        let value = samples
            .iter()
            .zip(&self.weights)
            .filter(|(_, weight)| **weight > 0.0)
            .map(|(sample, weight)| sample.value * *weight)
            .sum::<f64>()
            / total_weight;
        let threshold = samples
            .iter()
            .zip(&self.weights)
            .filter(|(_, weight)| **weight > 0.0)
            .map(|(sample, weight)| sample.threshold * *weight)
            .sum::<f64>()
            / total_weight;

        Score::new(value, threshold)
    }
}

fn mean_score(samples: &[Score]) -> Option<Score> {
    if samples.is_empty() {
        return None;
    }

    let count = samples.iter().fold(0.0, |count, _| count + 1.0);
    let value = samples.iter().map(|sample| sample.value).sum::<f64>() / count;
    let threshold = samples.iter().map(|sample| sample.threshold).sum::<f64>() / count;
    Some(Score::new(value, threshold))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
