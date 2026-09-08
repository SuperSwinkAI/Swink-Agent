//! Tests for `score`.
#![cfg(test)]

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
