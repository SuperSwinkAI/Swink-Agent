//! Tests for `match_`.
#![cfg(test)]

use super::*;
use serde_json::json;

fn recorded(name: &str, args: serde_json::Value) -> RecordedToolCall {
    RecordedToolCall {
        id: "id".to_string(),
        name: name.to_string(),
        arguments: args,
    }
}

fn expected(name: &str, args: Option<serde_json::Value>) -> ExpectedToolCall {
    ExpectedToolCall {
        tool_name: name.to_string(),
        arguments: args,
    }
}

#[test]
fn exact_match_all() {
    let exp = vec![
        expected("read", Some(json!({"path": "a.txt"}))),
        expected("write", None),
    ];
    let act = [
        recorded("read", json!({"path": "a.txt"})),
        recorded("write", json!({"path": "b.txt"})),
    ];
    let refs: Vec<&RecordedToolCall> = act.iter().collect();
    let (score, _) = score_exact(&exp, &refs);
    assert!((score.value - 1.0).abs() < f64::EPSILON);
}

#[test]
fn exact_match_wrong_order() {
    let exp = vec![expected("read", None), expected("write", None)];
    let act = [recorded("write", json!({})), recorded("read", json!({}))];
    let refs: Vec<&RecordedToolCall> = act.iter().collect();
    let (score, _) = score_exact(&exp, &refs);
    assert!((score.value - 0.0).abs() < f64::EPSILON);
}

#[test]
fn in_order_with_extras() {
    let exp = vec![expected("read", None), expected("write", None)];
    let act = [
        recorded("search", json!({})),
        recorded("read", json!({})),
        recorded("think", json!({})),
        recorded("write", json!({})),
    ];
    let refs: Vec<&RecordedToolCall> = act.iter().collect();
    let (score, _) = score_in_order(&exp, &refs);
    assert!((score.value - 1.0).abs() < f64::EPSILON);
}

#[test]
fn any_order_finds_all() {
    let exp = vec![expected("write", None), expected("read", None)];
    let act = [recorded("read", json!({})), recorded("write", json!({}))];
    let refs: Vec<&RecordedToolCall> = act.iter().collect();
    let (score, _) = score_any_order(&exp, &refs);
    assert!((score.value - 1.0).abs() < f64::EPSILON);
}

#[test]
fn any_order_partial_match() {
    let exp = vec![expected("read", None), expected("delete", None)];
    let act = [recorded("read", json!({})), recorded("write", json!({}))];
    let refs: Vec<&RecordedToolCall> = act.iter().collect();
    let (score, _) = score_any_order(&exp, &refs);
    assert!((score.value - 0.5).abs() < f64::EPSILON);
}
