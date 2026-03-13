//! Trajectory matching evaluator.
//!
//! Compares the actual tool call sequence against an expected golden path
//! using one of three matching modes.

use serde::{Deserialize, Serialize};

use crate::evaluator::Evaluator;
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, ExpectedToolCall, Invocation, RecordedToolCall};

/// How to compare actual tool calls against expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Same tools, same order, same count. No extras allowed.
    Exact,
    /// Expected tools must appear in order. Extra tools between are allowed.
    InOrder,
    /// All expected tools must appear somewhere. Order and extras don't matter.
    AnyOrder,
}

/// Evaluator that compares actual tool call trajectories against expected golden paths.
///
/// Returns `None` when the case has no `expected_trajectory`.
pub struct TrajectoryMatcher {
    mode: MatchMode,
}

impl TrajectoryMatcher {
    /// Create a matcher with the given mode.
    #[must_use]
    pub const fn new(mode: MatchMode) -> Self {
        Self { mode }
    }

    /// Exact matching: same tools, same order, same count.
    #[must_use]
    pub const fn exact() -> Self {
        Self::new(MatchMode::Exact)
    }

    /// In-order matching: expected tools appear in order, extras allowed.
    #[must_use]
    pub const fn in_order() -> Self {
        Self::new(MatchMode::InOrder)
    }

    /// Any-order matching: all expected tools appear, any order.
    #[must_use]
    pub const fn any_order() -> Self {
        Self::new(MatchMode::AnyOrder)
    }
}

impl Evaluator for TrajectoryMatcher {
    fn name(&self) -> &'static str {
        "trajectory"
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let expected = case.expected_trajectory.as_ref()?;

        // Flatten all actual tool calls across turns.
        let actual: Vec<&RecordedToolCall> = invocation
            .turns
            .iter()
            .flat_map(|t| &t.tool_calls)
            .collect();

        let (score, details) = match self.mode {
            MatchMode::Exact => score_exact(expected, &actual),
            MatchMode::InOrder => score_in_order(expected, &actual),
            MatchMode::AnyOrder => score_any_order(expected, &actual),
        };

        Some(EvalMetricResult {
            evaluator_name: "trajectory".to_string(),
            score,
            details: Some(details),
        })
    }
}

/// Check if a recorded tool call matches an expected one.
fn matches_expected(expected: &ExpectedToolCall, actual: &RecordedToolCall) -> bool {
    if expected.tool_name != actual.name {
        return false;
    }
    expected
        .arguments
        .as_ref()
        .is_none_or(|expected_args| *expected_args == actual.arguments)
}

/// Exact: same count, same order, each pair matches.
#[allow(clippy::cast_precision_loss)]
fn score_exact(expected: &[ExpectedToolCall], actual: &[&RecordedToolCall]) -> (Score, String) {
    if expected.len() != actual.len() {
        return (
            Score::new(0.0, 1.0),
            format!(
                "expected {} tool calls, got {}",
                expected.len(),
                actual.len()
            ),
        );
    }

    let matched = expected
        .iter()
        .zip(actual.iter())
        .filter(|(e, a)| matches_expected(e, a))
        .count();

    let total = expected.len().max(1);
    let value = matched as f64 / total as f64;
    let details = format!("{matched}/{total} tool calls matched exactly");
    (Score::new(value, 1.0), details)
}

/// In-order: expected tools appear in sequence, extras between are fine.
#[allow(clippy::cast_precision_loss)]
fn score_in_order(expected: &[ExpectedToolCall], actual: &[&RecordedToolCall]) -> (Score, String) {
    if expected.is_empty() {
        return (Score::pass(), "no expected tool calls".to_string());
    }

    let mut expected_idx = 0;
    for actual_call in actual {
        if expected_idx >= expected.len() {
            break;
        }
        if matches_expected(&expected[expected_idx], actual_call) {
            expected_idx += 1;
        }
    }

    let total = expected.len();
    let value = expected_idx as f64 / total as f64;
    let details = format!("{expected_idx}/{total} expected tool calls found in order");
    (Score::new(value, 1.0), details)
}

/// Any-order: each expected call must appear at least once.
#[allow(clippy::cast_precision_loss)]
fn score_any_order(expected: &[ExpectedToolCall], actual: &[&RecordedToolCall]) -> (Score, String) {
    if expected.is_empty() {
        return (Score::pass(), "no expected tool calls".to_string());
    }

    let matched = expected
        .iter()
        .filter(|e| actual.iter().any(|a| matches_expected(e, a)))
        .count();

    let total = expected.len();
    let value = matched as f64 / total as f64;
    let details = format!("{matched}/{total} expected tool calls found (any order)");
    (Score::new(value, 1.0), details)
}

#[cfg(test)]
mod tests {
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
}
