//! RAG-family evaluators (T066, T067).
//!
//! Three judge-backed evaluators score retrieval-augmented generation:
//!
//! * [`RAGGroundednessEvaluator`] — every claim in the response is supported
//!   by the retrieved context (prompt: `rag_groundedness_v0`).
//! * [`RAGRetrievalRelevanceEvaluator`] — the retrieved context is relevant
//!   to the user's prompt (prompt: `rag_retrieval_relevance_v0`).
//! * [`RAGHelpfulnessEvaluator`] — the response leverages the retrieved
//!   context to help the user (prompt: `rag_helpfulness_v0`).
//!
//! All three consume the retrieved context from `EvalCase::few_shot_examples`
//! — spec 043's canonical retrieval surface (see Quality family's
//! `FaithfulnessEvaluator` for the same convention).
//!
//! This module also ships the deterministic [`EmbeddingSimilarityEvaluator`]
//! and the [`Embedder`] trait + [`EmbedderError`] enum. The similarity
//! evaluator does NOT call a judge: it embeds the response and the reference
//! text via a caller-supplied [`Embedder`] implementation and scores by
//! cosine similarity, passing when similarity meets or exceeds the
//! configured threshold (default `0.8`).

#![forbid(unsafe_code)]
#![cfg(feature = "evaluator-rag")]

use std::sync::Arc;

use crate::evaluator::Evaluator;
use crate::score::Score;
use crate::types::{EvalCase, EvalMetricResult, Invocation};

use super::{JudgeEvaluatorConfig, build_prompt_context, evaluate_with_builtin};

fn has_final_response(invocation: &Invocation) -> bool {
    invocation
        .final_response
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
}

fn has_user_prompt(case: &EvalCase) -> bool {
    !case.user_messages.is_empty()
}

fn has_retrieved_context(case: &EvalCase) -> bool {
    !case.few_shot_examples.is_empty()
}

/// Macro for single-rubric RAG evaluators. Each evaluator's FR-020 criterion
/// is supplied as a closure; bodies dispatch via [`evaluate_with_builtin`].
macro_rules! rag_evaluator {
    (
        $(#[$meta:meta])*
        $name:ident, $eval_name:literal, $template:literal, $criterion:expr
    ) => {
        $(#[$meta])*
        pub struct $name {
            config: JudgeEvaluatorConfig,
        }

        impl $name {
            /// Construct with the supplied judge config.
            #[must_use]
            pub const fn new(config: JudgeEvaluatorConfig) -> Self {
                Self { config }
            }

            /// Override the prompt template used by this evaluator.
            #[must_use]
            pub fn with_prompt(mut self, template: Arc<dyn crate::prompt::JudgePromptTemplate>) -> Self {
                self.config = self.config.with_prompt(template);
                self
            }

            /// Attach evaluator-level few-shot examples that render before any
            /// case-level examples.
            #[must_use]
            pub fn with_few_shot(mut self, examples: Vec<crate::types::FewShotExample>) -> Self {
                self.config = self.config.with_few_shot(examples);
                self
            }

            /// Override the system prompt visible to the template render.
            #[must_use]
            pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
                self.config = self.config.with_system_prompt(prompt);
                self
            }

            /// Attach an output schema for custom prompt templates.
            #[must_use]
            pub fn with_output_schema(mut self, schema: serde_json::Value) -> Self {
                self.config = self.config.with_output_schema(schema);
                self
            }

            /// Toggle judge reasoning capture.
            #[must_use]
            pub fn with_use_reasoning(mut self, flag: bool) -> Self {
                self.config = self.config.with_use_reasoning(flag);
                self
            }

            /// Override the feedback key used by downstream exporters.
            #[must_use]
            pub fn with_feedback_key(mut self, key: impl Into<String>) -> Self {
                self.config = self.config.with_feedback_key(key);
                self
            }

            /// Borrow the underlying config (e.g., to inspect the judge
            /// registry or feedback key).
            #[must_use]
            pub const fn config(&self) -> &JudgeEvaluatorConfig {
                &self.config
            }
        }

        impl $crate::evaluators::JudgeEvaluatorBuilder for $name {
            fn judge_config_mut(&mut self) -> &mut JudgeEvaluatorConfig {
                &mut self.config
            }
        }

        impl Evaluator for $name {
            fn name(&self) -> &'static str {
                $eval_name
            }

            fn evaluate(
                &self,
                case: &EvalCase,
                invocation: &Invocation,
            ) -> Option<EvalMetricResult> {
                // FR-020: return None when the criterion is absent.
                let criterion: fn(&EvalCase, &Invocation) -> bool = $criterion;
                if !criterion(case, invocation) {
                    return None;
                }

                Some(evaluate_with_builtin(
                    $eval_name,
                    $template,
                    &self.config,
                    &build_prompt_context(&self.config, case, invocation),
                ))
            }
        }
    };
}

rag_evaluator! {
    /// Groundedness of the response against the retrieved context
    /// (prompt: `rag_groundedness_v0`).
    ///
    /// Criterion: the case must carry retrieved context
    /// (`few_shot_examples` non-empty), a user prompt, and a non-empty
    /// final response.
    RAGGroundednessEvaluator,
    "rag_groundedness",
    "rag_groundedness_v0",
    |case, invocation| has_retrieved_context(case)
        && has_user_prompt(case)
        && has_final_response(invocation)
}

rag_evaluator! {
    /// Relevance of the retrieved context to the user prompt
    /// (prompt: `rag_retrieval_relevance_v0`).
    ///
    /// Criterion: the case must carry retrieved context and a user prompt.
    /// A final response is not required — this rubric scores retrieval
    /// quality, not generation quality.
    RAGRetrievalRelevanceEvaluator,
    "rag_retrieval_relevance",
    "rag_retrieval_relevance_v0",
    |case, _invocation| has_retrieved_context(case) && has_user_prompt(case)
}

rag_evaluator! {
    /// Helpfulness of the response with respect to the retrieved context
    /// (prompt: `rag_helpfulness_v0`).
    ///
    /// Criterion: retrieved context, a user prompt, and a non-empty final
    /// response must all be present.
    RAGHelpfulnessEvaluator,
    "rag_helpfulness",
    "rag_helpfulness_v0",
    |case, invocation| has_retrieved_context(case)
        && has_user_prompt(case)
        && has_final_response(invocation)
}

// ─── Embedding similarity (deterministic, no judge) ─────────────────────────

/// Errors reported by an [`Embedder`] implementation.
///
/// Surfaced verbatim in [`EvalMetricResult::details`] when the
/// [`EmbeddingSimilarityEvaluator`] folds an embedding failure into
/// `Score::fail()` (FR-021 — the evaluator never crashes on a transport
/// hiccup).
#[derive(Debug, thiserror::Error)]
pub enum EmbedderError {
    /// Input text was rejected by the embedder (empty, too long, etc.).
    #[error("invalid input: {reason}")]
    InvalidInput {
        /// Human-readable explanation.
        reason: String,
    },
    /// The embedding backend failed (network, auth, quota, etc.).
    #[error("embedder backend error: {reason}")]
    Backend {
        /// Human-readable explanation.
        reason: String,
    },
    /// The returned vectors had mismatched dimensions.
    #[error("dimension mismatch: response={response_dim} reference={reference_dim}")]
    DimensionMismatch {
        /// Dimensionality of the response embedding.
        response_dim: usize,
        /// Dimensionality of the reference embedding.
        reference_dim: usize,
    },
}

/// Pluggable embedding backend used by [`EmbeddingSimilarityEvaluator`].
///
/// Implementations map a string to a dense vector. The evaluator only
/// consumes the trait — concrete backends (OpenAI, local model, stub) live
/// outside this crate.
pub trait Embedder: Send + Sync {
    /// Embed a single text into a dense vector.
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedderError>;
}

/// Cosine similarity between two vectors, clamped into `[-1.0, 1.0]`.
///
/// Returns `0.0` when either vector has zero magnitude.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot: f64 = 0.0;
    let mut na: f64 = 0.0;
    let mut nb: f64 = 0.0;
    for (x, y) in a.iter().zip(b.iter()) {
        let xf = f64::from(*x);
        let yf = f64::from(*y);
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    let sim = dot / (na.sqrt() * nb.sqrt());
    sim.clamp(-1.0, 1.0)
}

/// Default threshold for [`EmbeddingSimilarityEvaluator`] (cosine similarity).
pub const DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD: f64 = 0.8;

/// Deterministic cosine-similarity evaluator (T067).
///
/// Given a caller-supplied reference text and a caller-supplied [`Embedder`],
/// the evaluator:
///
/// 1. Returns `None` when the invocation has no final response (FR-020).
/// 2. Embeds both strings via the configured embedder.
/// 3. Computes cosine similarity, remaps it into `[0.0, 1.0]` via
///    `(sim + 1) / 2`, and emits a [`Score`] with the configured threshold.
///
/// Embedder failures fold into `Score::fail()` with the error message
/// recorded in `details` — panics are not possible from this codepath
/// because no user code runs synchronously beyond the trait call, which is
/// wrapped by the registry's `isolate_panic` guard.
pub struct EmbeddingSimilarityEvaluator {
    name: &'static str,
    reference: String,
    threshold: f64,
    embedder: Arc<dyn Embedder>,
}

impl EmbeddingSimilarityEvaluator {
    /// Construct with the given reference text and embedder.
    #[must_use]
    pub fn new(reference: impl Into<String>, embedder: Arc<dyn Embedder>) -> Self {
        Self {
            name: "embedding_similarity",
            reference: reference.into(),
            threshold: DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            embedder,
        }
    }

    /// Override the evaluator's reported name.
    #[must_use]
    pub const fn with_name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    /// Override the pass threshold applied to the remapped similarity score.
    ///
    /// The threshold is interpreted in `[0.0, 1.0]` — the evaluator remaps
    /// raw cosine similarity from `[-1.0, 1.0]` into `[0.0, 1.0]` before
    /// comparing against the threshold. Default is
    /// [`DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD`] (`0.8`).
    #[must_use]
    pub const fn with_threshold(mut self, threshold: f64) -> Self {
        self.threshold = threshold;
        self
    }

    /// Borrow the reference text.
    #[must_use]
    pub fn reference(&self) -> &str {
        &self.reference
    }

    /// The configured pass threshold.
    #[must_use]
    pub const fn threshold(&self) -> f64 {
        self.threshold
    }
}

impl Evaluator for EmbeddingSimilarityEvaluator {
    fn name(&self) -> &'static str {
        self.name
    }

    fn evaluate(&self, _case: &EvalCase, invocation: &Invocation) -> Option<EvalMetricResult> {
        // FR-020: criterion is a non-empty final response.
        let actual = invocation.final_response.as_deref()?;
        if actual.trim().is_empty() {
            return None;
        }

        let name = self.name.to_string();
        let a = match self.embedder.embed(actual) {
            Ok(v) => v,
            Err(err) => {
                return Some(EvalMetricResult {
                    evaluator_name: name,
                    score: Score::fail(),
                    details: Some(format!("embed_response: {err}")),
                });
            }
        };
        let b = match self.embedder.embed(&self.reference) {
            Ok(v) => v,
            Err(err) => {
                return Some(EvalMetricResult {
                    evaluator_name: name,
                    score: Score::fail(),
                    details: Some(format!("embed_reference: {err}")),
                });
            }
        };
        if a.len() != b.len() {
            let err = EmbedderError::DimensionMismatch {
                response_dim: a.len(),
                reference_dim: b.len(),
            };
            return Some(EvalMetricResult {
                evaluator_name: name,
                score: Score::fail(),
                details: Some(err.to_string()),
            });
        }

        let raw = cosine_similarity(&a, &b);
        // Remap cosine similarity from [-1, 1] into [0, 1] so the score
        // honours the Score::new clamp without collapsing negative values.
        let remapped = f64::midpoint(raw, 1.0).clamp(0.0, 1.0);
        let score = Score::new(remapped, self.threshold);
        Some(EvalMetricResult {
            evaluator_name: name,
            score,
            details: Some(format!(
                "cosine_similarity={raw:.4} remapped={remapped:.4} threshold={:.4}",
                self.threshold
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_of_identical_vectors_is_one() {
        let a = vec![1.0_f32, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn cosine_similarity_of_opposite_vectors_is_minus_one() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![-1.0_f32, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-9);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors_is_zero() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        assert!(cosine_similarity(&a, &b).abs() < 1e-9);
    }

    #[test]
    fn cosine_similarity_mismatched_dims_is_zero() {
        let a = vec![1.0_f32, 0.0];
        let b = vec![1.0_f32];
        assert!(cosine_similarity(&a, &b).abs() < 1e-9);
    }

    #[test]
    fn cosine_similarity_empty_vectors_is_zero() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        assert!(cosine_similarity(&a, &b).abs() < 1e-9);
    }
}
