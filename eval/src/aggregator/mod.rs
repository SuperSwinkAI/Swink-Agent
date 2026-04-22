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
#[derive(Debug, Clone, Copy, Default)]
pub struct Average;

impl Aggregator for Average {
    fn aggregate(&self, samples: &[Score]) -> Score {
        mean_score(samples).unwrap_or_default()
    }
}

/// Passes only when every sample passes.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllPass;

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
#[derive(Debug, Clone, Copy, Default)]
pub struct AnyPass;

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
#[derive(Debug, Clone, Default)]
pub struct Weighted {
    pub weights: Vec<f64>,
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
mod tests {
    use super::*;

    #[test]
    fn average_returns_mean_value_and_threshold() {
        let score = Average.aggregate(&[Score::new(0.2, 0.4), Score::new(0.8, 0.6)]);
        assert!((score.value - 0.5).abs() < f64::EPSILON);
        assert!((score.threshold - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn average_empty_is_default_score() {
        let score = Average.aggregate(&[]);
        assert!((score.value - 0.0).abs() < f64::EPSILON);
        assert!((score.threshold - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn all_pass_requires_every_sample_to_pass() {
        assert_eq!(
            AllPass
                .aggregate(&[Score::pass(), Score::new(0.7, 0.5)])
                .verdict(),
            crate::Verdict::Pass
        );
        assert_eq!(
            AllPass.aggregate(&[Score::pass(), Score::fail()]).verdict(),
            crate::Verdict::Fail
        );
    }

    #[test]
    fn any_pass_requires_one_passing_sample() {
        assert_eq!(
            AnyPass
                .aggregate(&[Score::fail(), Score::new(0.7, 0.5)])
                .verdict(),
            crate::Verdict::Pass
        );
        assert_eq!(
            AnyPass
                .aggregate(&[Score::fail(), Score::new(0.2, 0.9)])
                .verdict(),
            crate::Verdict::Fail
        );
    }

    #[test]
    fn weighted_uses_positive_weights() {
        let aggregator = Weighted {
            weights: vec![1.0, 3.0],
        };
        let score = aggregator.aggregate(&[Score::new(0.2, 0.4), Score::new(0.8, 0.6)]);
        assert!((score.value - 0.65).abs() < f64::EPSILON);
        assert!((score.threshold - 0.55).abs() < f64::EPSILON);
    }

    #[test]
    fn weighted_falls_back_to_average_for_mismatched_weights() {
        let aggregator = Weighted { weights: vec![1.0] };
        let score = aggregator.aggregate(&[Score::new(0.2, 0.4), Score::new(0.8, 0.6)]);
        assert!((score.value - 0.5).abs() < f64::EPSILON);
        assert!((score.threshold - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn weighted_falls_back_to_average_for_non_positive_total_weight() {
        let aggregator = Weighted {
            weights: vec![0.0, -1.0],
        };
        let score = aggregator.aggregate(&[Score::new(0.2, 0.4), Score::new(0.8, 0.6)]);
        assert!((score.value - 0.5).abs() < f64::EPSILON);
        assert!((score.threshold - 0.5).abs() < f64::EPSILON);
    }
}
