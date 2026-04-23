//! Structured-output evaluators (T072, T073 — structured family).
//!
//! Implements FR-016:
//! * [`JsonMatchEvaluator`] with per-key aggregation strategies and an
//!   `exclude_keys` filter. Rubric entries may dispatch to judge prompts,
//!   but the evaluator is itself deterministic when configured with
//!   `KeyStrategy::Average`/`All`/`None`.
//! * [`JsonSchemaEvaluator`] — deterministic JSON Schema validator built
//!   on the `jsonschema` crate; never dispatches a judge call.

use std::collections::HashSet;

use jsonschema::Validator;

use crate::evaluator::Evaluator;
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation};

/// Per-key aggregation strategy for [`JsonMatchEvaluator`].
#[derive(Debug, Clone)]
pub enum KeyStrategy {
    /// Average per-key scores (1.0 per matching key, 0.0 otherwise).
    Average,
    /// Pass only when every compared key matches the expected value.
    All,
    /// Pass only when none of the compared keys match (i.e. every key differs).
    None,
    /// Per-key rubric: keys listed here are scored by the caller-supplied
    /// closure; unlisted keys contribute a fixed default score.
    Rubric {
        /// Caller-supplied closure scoring a single key.
        ///
        /// Takes `(key, expected, actual)` and returns a score in `[0.0, 1.0]`.
        /// The closure is called only for keys present in the expected value.
        scorer: std::sync::Arc<
            dyn Fn(&str, &serde_json::Value, Option<&serde_json::Value>) -> f64 + Send + Sync,
        >,
    },
}

impl std::fmt::Debug for KeyStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Average => f.debug_tuple("Average").finish(),
            Self::All => f.debug_tuple("All").finish(),
            Self::None => f.debug_tuple("None").finish(),
            Self::Rubric { .. } => f.debug_struct("Rubric").field("scorer", &"<fn>").finish(),
        }
    }
}

/// JSON-match evaluator (FR-016).
///
/// Compares the invocation's `final_response` (parsed as JSON) against an
/// expected JSON value key-by-key. Keys present in `exclude_keys` are
/// skipped entirely. Aggregation across keys is controlled by [`KeyStrategy`].
pub struct JsonMatchEvaluator {
    name: &'static str,
    expected: serde_json::Value,
    strategy: KeyStrategy,
    exclude_keys: HashSet<String>,
}

impl JsonMatchEvaluator {
    /// Create a new evaluator with `KeyStrategy::Average` semantics and no
    /// excluded keys.
    #[must_use]
    pub fn new(expected: serde_json::Value) -> Self {
        Self {
            name: "json_match",
            expected,
            strategy: KeyStrategy::Average,
            exclude_keys: HashSet::new(),
        }
    }

    /// Override the evaluator's reported name.
    #[must_use]
    pub const fn with_name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Override the per-key aggregation strategy.
    #[must_use]
    pub fn with_strategy(mut self, strategy: KeyStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Exclude the named keys from comparison.
    #[must_use]
    pub fn with_exclude_keys<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.exclude_keys = keys.into_iter().map(Into::into).collect();
        self
    }

    fn compare(&self, actual: &serde_json::Value) -> (f64, String) {
        let expected_obj = match self.expected.as_object() {
            Some(obj) => obj,
            None => {
                // Scalar / array comparison falls back to full equality.
                let eq = self.expected == *actual;
                return (
                    if eq { 1.0 } else { 0.0 },
                    if eq {
                        "match".into()
                    } else {
                        "mismatch".into()
                    },
                );
            }
        };
        let actual_obj = actual.as_object();

        let mut per_key: Vec<(String, f64)> = Vec::new();
        for (key, expected_value) in expected_obj {
            if self.exclude_keys.contains(key) {
                continue;
            }
            let actual_value = actual_obj.and_then(|obj| obj.get(key));
            let score = match &self.strategy {
                KeyStrategy::Average | KeyStrategy::All | KeyStrategy::None => {
                    if actual_value == Some(expected_value) {
                        1.0
                    } else {
                        0.0
                    }
                }
                KeyStrategy::Rubric { scorer } => {
                    scorer(key, expected_value, actual_value).clamp(0.0_f64, 1.0_f64)
                }
            };
            per_key.push((key.clone(), score));
        }

        if per_key.is_empty() {
            return (1.0, "no comparable keys".into());
        }

        let score = match &self.strategy {
            KeyStrategy::Average | KeyStrategy::Rubric { .. } => {
                let sum: f64 = per_key.iter().map(|(_, s)| *s).sum();
                #[allow(clippy::cast_precision_loss)]
                {
                    sum / per_key.len() as f64
                }
            }
            KeyStrategy::All => {
                if per_key.iter().all(|(_, s)| *s >= 1.0) {
                    1.0
                } else {
                    0.0
                }
            }
            KeyStrategy::None => {
                if per_key.iter().all(|(_, s)| *s <= 0.0) {
                    1.0
                } else {
                    0.0
                }
            }
        };

        let details = per_key
            .iter()
            .map(|(k, s)| format!("{k}={s:.2}"))
            .collect::<Vec<_>>()
            .join(", ");
        (score, details)
    }
}

impl Evaluator for JsonMatchEvaluator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn evaluate(&self, _case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let raw = invocation.final_response.as_ref()?;
        let parsed: serde_json::Value = match serde_json::from_str(raw) {
            Ok(value) => value,
            Err(err) => {
                return Some(EvalMetricResult {
                    evaluator_name: self.name.to_string(),
                    score: Score::fail(),
                    details: Some(format!("malformed JSON response: {err}")),
                });
            }
        };

        let (value, details) = self.compare(&parsed);
        Some(EvalMetricResult {
            evaluator_name: self.name.to_string(),
            score: Score::new(value, 0.5),
            details: Some(details),
        })
    }
}

/// Deterministic JSON-schema evaluator (FR-016 / T073).
///
/// Compiles the configured schema once at construction time using the
/// `jsonschema` crate; evaluation parses the invocation's `final_response`
/// and runs the compiled validator. No judge call is ever dispatched.
pub struct JsonSchemaEvaluator {
    name: &'static str,
    validator: Validator,
}

impl JsonSchemaEvaluator {
    /// Compile the schema and return a ready-to-use evaluator. Returns an
    /// error when the schema itself is invalid.
    pub fn new(schema: &serde_json::Value) -> Result<Self, String> {
        let validator = jsonschema::validator_for(schema).map_err(|err| err.to_string())?;
        Ok(Self {
            name: "json_schema",
            validator,
        })
    }

    /// Override the evaluator's reported name.
    #[must_use]
    pub const fn with_name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }
}

impl Evaluator for JsonSchemaEvaluator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn evaluate(&self, _case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        let raw = invocation.final_response.as_ref()?;
        let parsed: serde_json::Value = match serde_json::from_str(raw) {
            Ok(value) => value,
            Err(err) => {
                return Some(EvalMetricResult {
                    evaluator_name: self.name.to_string(),
                    score: Score::fail(),
                    details: Some(format!("malformed JSON response: {err}")),
                });
            }
        };

        let errors: Vec<String> = self
            .validator
            .iter_errors(&parsed)
            .map(|err| err.to_string())
            .collect();

        if errors.is_empty() {
            Some(EvalMetricResult {
                evaluator_name: self.name.to_string(),
                score: Score::pass(),
                details: Some("schema valid".into()),
            })
        } else {
            Some(EvalMetricResult {
                evaluator_name: self.name.to_string(),
                score: Score::fail(),
                details: Some(errors.join("; ")),
            })
        }
    }
}
