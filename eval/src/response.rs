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
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct ResponseMatcher;

impl ResponseMatcher {
    /// Create a new `ResponseMatcher`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

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
#[path = "response_tests.rs"]
mod tests;
