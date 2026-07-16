//! Integration tests for Score and Verdict.

use swink_agent_eval::{Score, Verdict};

#[test]
fn score_serde_roundtrip() {
    let score = Score::new(0.75, 0.6);
    let json = serde_json::to_string(&score).unwrap();
    let parsed: Score = serde_json::from_str(&json).unwrap();
    assert!((parsed.value - 0.75).abs() < f64::EPSILON);
    assert!((parsed.threshold - 0.6).abs() < f64::EPSILON);
}

#[test]
fn verdict_serde_roundtrip() {
    let json = serde_json::to_string(&Verdict::Pass).unwrap();
    assert_eq!(json, "\"pass\"");
    let parsed: Verdict = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, Verdict::Pass);
}
