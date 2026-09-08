//! Environment-state assertion evaluator.
//!
//! Compares named environment-state snapshots captured after an agent run
//! against the expected values declared on the eval case.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::evaluator::Evaluator;
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation};

/// Deterministic evaluator for environment-side effects.
///
/// Returns `None` when either `expected_environment_state` or `state_capture`
/// is absent on the case.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct EnvironmentStateEvaluator;

impl EnvironmentStateEvaluator {
    /// Create a new `EnvironmentStateEvaluator`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Evaluator for EnvironmentStateEvaluator {
    fn name(&self) -> &'static str {
        "environment_state"
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let expected_states = case.expected_environment_state.as_ref()?;
        let state_capture = case.state_capture.as_ref()?;

        let actual_states = match catch_unwind(AssertUnwindSafe(|| state_capture(invocation))) {
            Ok(states) => states,
            Err(payload) => {
                return Some(EvalMetricResult {
                    evaluator_name: self.name().to_string(),
                    score: Score::fail(),
                    details: Some(format!(
                        "state capture panicked: {}",
                        panic_payload_message(payload.as_ref())
                    )),
                });
            }
        };

        let actual_by_name: HashMap<&str, &serde_json::Value> = actual_states
            .iter()
            .map(|state| (state.name.as_str(), &state.state))
            .collect();

        for expected in expected_states {
            let Some(actual) = actual_by_name.get(expected.name.as_str()) else {
                return Some(EvalMetricResult {
                    evaluator_name: self.name().to_string(),
                    score: Score::fail(),
                    details: Some(format!(
                        "missing expected environment state `{}`",
                        expected.name
                    )),
                });
            };

            if *actual != &expected.state {
                return Some(EvalMetricResult {
                    evaluator_name: self.name().to_string(),
                    score: Score::fail(),
                    details: Some(format!(
                        "environment state `{}` mismatch: expected {}, actual {}",
                        expected.name, expected.state, actual
                    )),
                });
            }
        }

        let matched_names = expected_states
            .iter()
            .map(|state| state.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        Some(EvalMetricResult {
            evaluator_name: self.name().to_string(),
            score: Score::pass(),
            details: Some(format!("matched environment states: {matched_names}")),
        })
    }
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

#[cfg(test)]
#[path = "environment_state_tests.rs"]
mod tests;
