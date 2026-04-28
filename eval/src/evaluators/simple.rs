//! Simple deterministic evaluators (T075 — simple family).
//!
//! These evaluators do not involve a judge dispatch; they compare the
//! agent's final response string against a caller-supplied expected value
//! using exact equality or normalized Levenshtein similarity. They satisfy
//! FR-018 and the FR-020 "`None` when criterion not set" convention.

use strsim::levenshtein;

use crate::evaluator::Evaluator;
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation};

/// Exact-match evaluator (FR-018).
///
/// Compares the invocation's `final_response` against a configured expected
/// value. Returns `None` when no final response is produced.
pub struct ExactMatchEvaluator {
    name: &'static str,
    expected: String,
    case_sensitive: bool,
    trim: bool,
}

impl ExactMatchEvaluator {
    /// Create an evaluator whose `name()` defaults to `"exact_match"`.
    #[must_use]
    pub fn new(expected: impl Into<String>) -> Self {
        Self {
            name: "exact_match",
            expected: expected.into(),
            case_sensitive: true,
            trim: false,
        }
    }

    /// Override the evaluator's reported name (useful when a case wires
    /// multiple expected-value evaluators).
    #[must_use]
    pub const fn with_name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Toggle case sensitivity. Defaults to case-sensitive.
    #[must_use]
    pub const fn case_sensitive(mut self, case_sensitive: bool) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    /// Toggle whitespace trimming before comparison. Defaults to `false`.
    #[must_use]
    pub const fn trim(mut self, trim: bool) -> Self {
        self.trim = trim;
        self
    }

    fn normalize<'a>(&self, text: &'a str) -> std::borrow::Cow<'a, str> {
        let trimmed = if self.trim { text.trim() } else { text };
        if self.case_sensitive {
            std::borrow::Cow::Borrowed(trimmed)
        } else {
            std::borrow::Cow::Owned(trimmed.to_lowercase())
        }
    }
}

impl Evaluator for ExactMatchEvaluator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn evaluate(&self, _case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let actual = invocation.final_response.as_ref()?;
        let actual_norm = self.normalize(actual);
        let expected_norm = self.normalize(&self.expected);
        let matched = actual_norm == expected_norm;
        let score = if matched {
            Score::pass()
        } else {
            Score::fail()
        };
        Some(EvalMetricResult {
            evaluator_name: self.name.to_string(),
            score,
            details: Some(if matched {
                "match".to_string()
            } else {
                format!(
                    "expected `{}`, got `{}`",
                    expected_norm.as_ref(),
                    actual_norm.as_ref()
                )
            }),
        })
    }
}

/// Levenshtein-distance evaluator (FR-018).
///
/// Produces a normalized similarity score in `[0.0, 1.0]` computed as
/// `1.0 - distance / max(len(expected), len(actual))`. Returns `None` when
/// no final response is produced.
pub struct LevenshteinDistanceEvaluator {
    name: &'static str,
    expected: String,
    /// Pass-threshold applied to the normalized similarity score.
    threshold: f64,
}

impl LevenshteinDistanceEvaluator {
    /// Create an evaluator whose `name()` defaults to `"levenshtein_distance"`.
    ///
    /// The default threshold is `0.8` — callers can tune per-case via
    /// [`Self::with_threshold`].
    #[must_use]
    pub fn new(expected: impl Into<String>) -> Self {
        Self {
            name: "levenshtein_distance",
            expected: expected.into(),
            threshold: 0.8,
        }
    }

    /// Override the evaluator's reported name.
    #[must_use]
    pub const fn with_name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Override the pass threshold for the normalized similarity score.
    #[must_use]
    pub const fn with_threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }
}

impl Evaluator for LevenshteinDistanceEvaluator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn evaluate(&self, _case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let actual = invocation.final_response.as_ref()?;
        let distance = levenshtein(&self.expected, actual);
        let max_len = self.expected.chars().count().max(actual.chars().count());
        let similarity = if max_len == 0 {
            1.0_f64
        } else {
            #[allow(clippy::cast_precision_loss)]
            {
                1.0_f64 - (distance as f64 / max_len as f64)
            }
        };
        let score = Score::new(similarity, self.threshold);
        Some(EvalMetricResult {
            evaluator_name: self.name.to_string(),
            score,
            details: Some(format!(
                "distance={distance} similarity={similarity:.3} threshold={:.3}",
                self.threshold
            )),
        })
    }
}
