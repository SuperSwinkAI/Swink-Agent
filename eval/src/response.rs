//! Response matching evaluator.
//!
//! Scores the agent's final response text against expected criteria:
//! exact match, substring containment, regex pattern, or custom function.

use std::panic::{AssertUnwindSafe, catch_unwind};

use regex::Regex;
use swink_agent::prefix_chars;

use crate::evaluator::Evaluator;
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation, ResponseCriteria};

/// Evaluator that scores the final response text against expected criteria.
///
/// Returns `None` when the case has no `expected_response` defined.
pub struct ResponseMatcher;

impl Evaluator for ResponseMatcher {
    fn name(&self) -> &'static str {
        "response"
    }

    fn evaluate(&self, case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let criteria = case.expected_response.as_ref()?;
        let actual = invocation.final_response.as_deref().unwrap_or("");

        let (score, details) = match criteria {
            ResponseCriteria::Exact { expected } => {
                if actual == expected {
                    (Score::pass(), "exact match".to_string())
                } else {
                    (
                        Score::fail(),
                        format!("expected exact match, got: {}", truncate(actual, 100)),
                    )
                }
            }
            ResponseCriteria::Contains { substring } => {
                if actual.contains(substring.as_str()) {
                    (Score::pass(), format!("contains \"{substring}\""))
                } else {
                    (
                        Score::fail(),
                        format!(
                            "expected to contain \"{substring}\", got: {}",
                            truncate(actual, 100)
                        ),
                    )
                }
            }
            ResponseCriteria::Regex { pattern } => match Regex::new(pattern) {
                Ok(re) => {
                    if re.is_match(actual) {
                        (Score::pass(), format!("matches pattern /{pattern}/"))
                    } else {
                        (
                            Score::fail(),
                            format!("does not match /{pattern}/, got: {}", truncate(actual, 100)),
                        )
                    }
                }
                Err(e) => (Score::fail(), format!("invalid regex: {e}")),
            },
            ResponseCriteria::Custom(f) => match catch_unwind(AssertUnwindSafe(|| f(actual))) {
                Ok(score) => {
                    let details = format!("custom score: {:.2}", score.value);
                    (score, details)
                }
                Err(payload) => {
                    let msg = payload
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    (Score::fail(), format!("custom matcher panicked: {msg}"))
                }
            },
        };

        Some(EvalMetricResult {
            evaluator_name: "response".to_string(),
            score,
            details: Some(details),
        })
    }
}

/// Truncate a string to at most `max_len` characters, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        format!("{}...", prefix_chars(s, max_len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::time::Duration;

    use swink_agent::{AssistantMessage, ContentBlock, Cost, ModelSpec, StopReason, Usage};

    use crate::types::{EvalCase, Invocation, TurnRecord};

    fn minimal_case_with_response(criteria: ResponseCriteria) -> EvalCase {
        EvalCase {
            id: "test".to_string(),
            name: "Test".to_string(),
            description: None,
            system_prompt: "test".to_string(),
            user_messages: vec!["test".to_string()],
            expected_trajectory: None,
            expected_response: Some(criteria),
            budget: None,
            evaluators: vec![],
            metadata: serde_json::Value::Null,
            expected_environment_state: None,
            expected_tool_intent: None,
            semantic_tool_selection: false,
            state_capture: None,
        }
    }

    fn invocation_with_response(text: &str) -> Invocation {
        Invocation {
            turns: vec![TurnRecord {
                turn_index: 0,
                assistant_message: AssistantMessage {
                    content: vec![ContentBlock::Text {
                        text: text.to_string(),
                    }],
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
                duration: Duration::from_millis(10),
            }],
            total_usage: Usage::default(),
            total_cost: Cost::default(),
            total_duration: Duration::from_millis(10),
            final_response: Some(text.to_string()),
            stop_reason: StopReason::Stop,
            model: ModelSpec::new("test", "test-model"),
        }
    }

    #[test]
    fn truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(200);
        let result = truncate(&long, 100);
        assert_eq!(result.len(), 103); // 100 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_multibyte_string_is_utf8_safe() {
        let text = format!("{}🙂tail", "a".repeat(99));
        let result = truncate(&text, 100);
        assert_eq!(result, format!("{}🙂...", "a".repeat(99)));
    }

    #[test]
    fn custom_fn_panic_caught_as_failure() {
        let criteria = ResponseCriteria::Custom(Arc::new(|_: &str| -> Score {
            panic!("deliberate test panic");
        }));
        let case = minimal_case_with_response(criteria);
        let invocation = invocation_with_response("anything");

        let result = ResponseMatcher.evaluate(&case, &invocation).unwrap();
        assert!((result.score.value - 0.0).abs() < f64::EPSILON);
        let details = result.details.unwrap();
        assert!(
            details.contains("panicked"),
            "expected panic mention, got: {details}"
        );
        assert!(
            details.contains("deliberate test panic"),
            "expected panic message, got: {details}"
        );
    }
}
