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
pub struct EnvironmentStateEvaluator;

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
mod tests {
    use std::sync::Arc;

    use swink_agent::{Cost, ModelSpec, StopReason, Usage};

    use super::*;
    use crate::types::{EnvironmentState, TurnRecord};

    fn case_with_state_capture() -> EvalCase {
        EvalCase {
            id: "env".into(),
            name: "Environment".into(),
            description: None,
            system_prompt: "test".into(),
            user_messages: vec!["hi".into()],
            expected_trajectory: None,
            expected_response: None,
            budget: None,
            evaluators: vec![],
            metadata: serde_json::Value::Null,
            attachments: vec![],
            expected_environment_state: Some(vec![EnvironmentState {
                name: "created_file".into(),
                state: serde_json::json!("out.md"),
            }]),
            expected_tool_intent: None,
            semantic_tool_selection: false,
            state_capture: Some(Arc::new(|_| {
                vec![EnvironmentState {
                    name: "created_file".into(),
                    state: serde_json::json!("out.md"),
                }]
            })),
        }
    }

    fn empty_invocation() -> Invocation {
        Invocation {
            turns: vec![TurnRecord {
                turn_index: 0,
                assistant_message: swink_agent::AssistantMessage {
                    content: vec![],
                    provider: "test".into(),
                    model_id: "test-model".into(),
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
                duration: std::time::Duration::from_millis(10),
            }],
            total_usage: Usage::default(),
            total_cost: Cost::default(),
            total_duration: std::time::Duration::from_millis(10),
            final_response: None,
            stop_reason: StopReason::Stop,
            model: ModelSpec::new("test", "test-model"),
        }
    }

    #[test]
    fn returns_none_without_expected_states() {
        let mut case = case_with_state_capture();
        case.expected_environment_state = None;
        assert!(
            EnvironmentStateEvaluator
                .evaluate(&case, &empty_invocation())
                .is_none()
        );
    }

    #[test]
    fn returns_none_without_state_capture() {
        let mut case = case_with_state_capture();
        case.state_capture = None;
        assert!(
            EnvironmentStateEvaluator
                .evaluate(&case, &empty_invocation())
                .is_none()
        );
    }

    #[test]
    fn matching_state_passes() {
        let result = EnvironmentStateEvaluator
            .evaluate(&case_with_state_capture(), &empty_invocation())
            .expect("evaluator should apply");

        assert!(result.score.verdict().is_pass());
        assert!(
            result
                .details
                .as_deref()
                .is_some_and(|details| details.contains("created_file"))
        );
    }
}
