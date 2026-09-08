//! Tests for `mod`.
#![cfg(test)]

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
