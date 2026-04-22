use swink_agent_eval::{Aggregator, AllPass, AnyPass, Average, Score, Verdict, Weighted};

#[test]
fn average_aggregator_uses_arithmetic_mean() {
    let score = Average.aggregate(&[Score::new(0.25, 0.5), Score::new(0.75, 0.5)]);
    assert!((score.value - 0.5).abs() < f64::EPSILON);
    assert_eq!(score.verdict(), Verdict::Pass);
}

#[test]
fn all_pass_aggregator_returns_binary_failure_when_any_sample_fails() {
    let score = AllPass.aggregate(&[Score::pass(), Score::new(0.1, 0.5)]);
    assert_eq!(score.verdict(), Verdict::Fail);
    assert!((score.value - 0.0).abs() < f64::EPSILON);
}

#[test]
fn any_pass_aggregator_returns_binary_pass_when_one_sample_passes() {
    let score = AnyPass.aggregate(&[Score::new(0.1, 0.5), Score::new(0.9, 0.5)]);
    assert_eq!(score.verdict(), Verdict::Pass);
    assert!((score.value - 1.0).abs() < f64::EPSILON);
}

#[test]
fn weighted_aggregator_uses_configured_weights() {
    let aggregator = Weighted {
        weights: vec![1.0, 2.0, 1.0],
    };
    let score = aggregator.aggregate(&[
        Score::new(0.2, 0.5),
        Score::new(0.6, 0.5),
        Score::new(1.0, 0.5),
    ]);

    assert!((score.value - 0.6).abs() < f64::EPSILON);
    assert!((score.threshold - 0.5).abs() < f64::EPSILON);
}
