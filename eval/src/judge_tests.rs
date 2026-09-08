//! Tests for `judge`.
#![cfg(test)]

use super::*;

#[test]
fn judge_error_display_variants() {
    assert_eq!(
        JudgeError::Transport("boom".into()).to_string(),
        "transport: boom"
    );
    assert_eq!(JudgeError::Timeout.to_string(), "timeout");
    assert_eq!(
        JudgeError::MalformedResponse("bad".into()).to_string(),
        "malformed response: bad"
    );
    assert_eq!(
        JudgeError::Other("thing".into()).to_string(),
        "other: thing"
    );
}

#[test]
fn verdict_fields_are_public() {
    let v = JudgeVerdict {
        score: 0.75,
        pass: true,
        reason: Some("looks right".into()),
        label: Some("equivalent".into()),
        cost: Some(0.002),
    };
    assert!((v.score - 0.75).abs() < f64::EPSILON);
    assert!(v.pass);
    assert_eq!(v.reason.as_deref(), Some("looks right"));
    assert_eq!(v.label.as_deref(), Some("equivalent"));
    assert_eq!(v.cost, Some(0.002));
}

#[test]
fn verdict_cost_defaults_to_none_when_omitted_from_json() {
    let json = r#"{"score":1.0,"pass":true,"reason":null,"label":null}"#;
    let v: JudgeVerdict = serde_json::from_str(json).unwrap();
    assert_eq!(v.cost, None);
}
