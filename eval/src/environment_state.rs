//! Environment-state assertion evaluator.
//!
//! Compares a captured snapshot of environment state (produced by a
//! [`StateCapture`](crate::types::StateCapture) closure registered on the
//! [`EvalCase`]) against the case's `expected_environment_state` via full
//! JSON equality (FR-013, FR-014, FR-015).
//!
//! Deterministic — no LLM dependency — so it is registered unconditionally in
//! [`EvaluatorRegistry::with_defaults()`](crate::EvaluatorRegistry::with_defaults).
//! The evaluator returns `None` when either the capture callback or the
//! expected list is absent, so default registration is inert for cases that
//! do not configure env-state assertions.

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::evaluator::Evaluator;
use crate::score::Score;
use crate::types::{EnvironmentState, EvalCase, EvalMetricResult, Invocation};

/// Name reported via [`Evaluator::name`] and used in
/// [`EvalCase::evaluators`](crate::EvalCase::evaluators) filters.
const EVALUATOR_NAME: &str = "environment_state";

/// Deterministic env-state evaluator.
///
/// See module docs for the full behavioral contract. In short:
///
/// * Returns `None` when `case.expected_environment_state` is absent OR
///   `case.state_capture` is absent.
/// * Otherwise runs the capture closure under
///   [`std::panic::catch_unwind`] — a panicking closure becomes
///   [`Score::fail()`] with the panic message in details (FR-014).
/// * For each expected [`EnvironmentState`], looks up the captured value by
///   name and compares via full JSON equality. Missing names and value
///   mismatches are failures; extra captured names are ignored (the expected
///   list is a subset-check, per the US7 edge-case list).
pub struct EnvironmentStateEvaluator;

impl Evaluator for EnvironmentStateEvaluator {
    fn name(&self) -> &'static str {
        EVALUATOR_NAME
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let expected = case.expected_environment_state.as_ref()?;
        let capture = case.state_capture.as_ref()?;

        let captured = match catch_unwind(AssertUnwindSafe(|| capture(invocation))) {
            Ok(states) => states,
            Err(payload) => {
                let msg = panic_message(payload.as_ref());
                return Some(result_fail(format!("state_capture panicked: {msg}")));
            }
        };

        for expected_state in expected {
            let Some(actual) = captured
                .iter()
                .find(|entry| entry.name == expected_state.name)
            else {
                return Some(result_fail(format!(
                    "missing expected environment state: `{}`",
                    expected_state.name
                )));
            };

            if actual.state != expected_state.state {
                let expected_json = serde_json::to_string(&expected_state.state)
                    .unwrap_or_else(|_| "<unserializable>".into());
                let actual_json = serde_json::to_string(&actual.state)
                    .unwrap_or_else(|_| "<unserializable>".into());
                return Some(result_fail(format!(
                    "environment state `{name}` mismatch: expected {expected_json}, actual {actual_json}",
                    name = expected_state.name,
                )));
            }
        }

        Some(result_pass(expected))
    }
}

fn result_pass(expected: &[EnvironmentState]) -> EvalMetricResult {
    let names: Vec<&str> = expected.iter().map(|e| e.name.as_str()).collect();
    EvalMetricResult {
        evaluator_name: EVALUATOR_NAME.to_string(),
        score: Score::pass(),
        details: Some(format!(
            "matched environment states: [{}]",
            names.join(", ")
        )),
    }
}

fn result_fail(details: String) -> EvalMetricResult {
    EvalMetricResult {
        evaluator_name: EVALUATOR_NAME.to_string(),
        score: Score::fail(),
        details: Some(details),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::time::Duration;

    use swink_agent::{AssistantMessage, Cost, ModelSpec, StopReason, Usage};

    use crate::types::{EvalCase, Invocation, TurnRecord};

    fn empty_invocation() -> Invocation {
        Invocation {
            turns: vec![TurnRecord {
                turn_index: 0,
                assistant_message: AssistantMessage {
                    content: vec![],
                    provider: "test".to_string(),
                    model_id: "test-model".to_string(),
                    usage: Usage::default(),
                    cost: Cost::default(),
                    stop_reason: StopReason::Stop,
                    error_message: None,
                    error_kind: None,
                    timestamp: 0,
                    cache_hint: None,
                },
                tool_calls: vec![],
                tool_results: vec![],
                duration: Duration::from_millis(1),
            }],
            total_usage: Usage::default(),
            total_cost: Cost::default(),
            total_duration: Duration::from_millis(1),
            final_response: None,
            stop_reason: StopReason::Stop,
            model: ModelSpec::new("test", "test-model"),
        }
    }

    fn base_case() -> EvalCase {
        EvalCase {
            id: "c".into(),
            name: "c".into(),
            description: None,
            system_prompt: String::new(),
            user_messages: vec!["hi".into()],
            expected_trajectory: None,
            expected_response: None,
            budget: None,
            evaluators: vec![],
            metadata: serde_json::Value::Null,
            expected_environment_state: None,
            expected_tool_intent: None,
            semantic_tool_selection: false,
            state_capture: None,
        }
    }

    #[test]
    fn returns_none_without_expected_or_capture() {
        let case = base_case();
        let inv = empty_invocation();
        assert!(EnvironmentStateEvaluator.evaluate(&case, &inv).is_none());
    }

    #[test]
    fn returns_none_with_expected_but_no_capture() {
        let mut case = base_case();
        case.expected_environment_state = Some(vec![EnvironmentState {
            name: "x".into(),
            state: serde_json::json!({}),
        }]);
        let inv = empty_invocation();
        assert!(EnvironmentStateEvaluator.evaluate(&case, &inv).is_none());
    }

    #[test]
    fn returns_none_with_capture_but_no_expected() {
        let mut case = base_case();
        case.state_capture = Some(Arc::new(|_| vec![]));
        let inv = empty_invocation();
        assert!(EnvironmentStateEvaluator.evaluate(&case, &inv).is_none());
    }
}
