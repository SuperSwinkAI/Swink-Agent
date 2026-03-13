//! Response matching evaluator.
//!
//! Scores the agent's final response text against expected criteria:
//! exact match, substring containment, regex pattern, or custom function.

use regex::Regex;

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
            ResponseCriteria::Custom(f) => {
                let score = f(actual);
                let details = format!("custom score: {:.2}", score.value);
                (score, details)
            }
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
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
