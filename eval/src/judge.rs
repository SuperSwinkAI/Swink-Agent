//! LLM-as-judge client trait for semantic evaluators.
//!
//! `JudgeClient` is an async trait consumed by the semantic evaluators
//! (`SemanticToolSelectionEvaluator`, `SemanticToolParameterEvaluator`).
//!
//! # Scope: trait + test doubles only
//!
//! Spec 023 ships **only** the trait shape, the [`JudgeVerdict`] /
//! [`JudgeError`] types, and a pair of test doubles
//! ([`crate::MockJudge`], [`crate::SlowMockJudge`]). Concrete provider-backed
//! [`JudgeClient`] implementations — Anthropic, `OpenAI`, Gemini, Azure, local
//! models, prompt-template registries, retry / backoff, batching, caching —
//! are explicitly out of scope and will ship in spec 043
//! (`specs/043-evals-adv-features`) in the companion
//! `swink-agent-eval-judges` crate. Until then, downstream consumers wire up
//! their own [`JudgeClient`] impl (often a thin wrapper around an existing
//! `swink-agent` provider handle) and pass it to
//! [`crate::EvaluatorRegistry::with_defaults_and_judge`].
//!
//! # Non-hang guarantee
//!
//! The trait has no deadline parameter; deadline enforcement is evaluator-side
//! via `tokio::time::timeout` (default 5 min, configurable per evaluator).
//! See `FR-010` and `FR-014` in `specs/023-eval-trajectory-matching/spec.md`.
//!
//! # Error mapping
//!
//! All [`JudgeError`] variants map to `Score::fail()` with the variant name
//! and context captured in `EvalMetricResult.details`.

#![forbid(unsafe_code)]

use async_trait::async_trait;

pub use crate::url_filter::{DefaultUrlFilter, UrlFilter};

/// LLM-as-judge client used by semantic evaluators.
///
/// The trait exposes a single async method that accepts a rendered prompt and
/// returns a structured [`JudgeVerdict`]. Concrete implementations (model
/// providers, prompt templating, retry / backoff, batching) are explicitly out
/// of scope for spec 023.
#[async_trait]
pub trait JudgeClient: Send + Sync {
    /// Judge the given prompt and return a structured verdict.
    ///
    /// Implementations MAY enforce their own inner deadlines and surface
    /// them via [`JudgeError::Timeout`]. Evaluators additionally wrap each
    /// call in an outer `tokio::time::timeout`.
    async fn judge(&self, prompt: &str) -> Result<JudgeVerdict, JudgeError>;
}

/// Structured verdict returned by a [`JudgeClient`].
///
/// Mirrors `strands-evals::EvaluationOutput` so future provider bindings can
/// map cleanly without provider-specific types leaking into this crate.
#[derive(Debug, Clone)]
pub struct JudgeVerdict {
    /// Numeric score in `[0.0, 1.0]`. Callers SHOULD clamp before constructing.
    pub score: f64,
    /// Judge's own pass/fail determination; evaluators surface this directly
    /// in the `EvalMetricResult`.
    pub pass: bool,
    /// Human-readable justification, surfaced in `EvalMetricResult.details`.
    pub reason: Option<String>,
    /// Optional category label (e.g., "equivalent", "unrelated").
    pub label: Option<String>,
}

/// Error type returned by [`JudgeClient::judge`].
///
/// Semantic evaluators map every variant to `Score::fail()` with the variant
/// name and context captured in `EvalMetricResult.details` (FR-014).
#[derive(Debug, thiserror::Error)]
pub enum JudgeError {
    /// Network or transport failure (connection refused, DNS error, etc.).
    #[error("transport: {0}")]
    Transport(String),
    /// An inner deadline fired inside the concrete [`JudgeClient`] impl.
    #[error("timeout")]
    Timeout,
    /// Response parsed successfully but violates the verdict schema.
    #[error("malformed response: {0}")]
    MalformedResponse(String),
    /// Catch-all with a diagnostic string for anything outside the above.
    #[error("other: {0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn judge_error_display_variants() {
        assert_eq!(
            JudgeError::Transport("boom".into()).to_string(),
            "transport: boom"
        );
        assert_eq!(JudgeError::Timeout.to_string(), "timeout");
        assert_eq!(
            JudgeError::MalformedResponse("bad".into()).to_string(),
            "malformed response: bad"
        );
        assert_eq!(
            JudgeError::Other("thing".into()).to_string(),
            "other: thing"
        );
    }

    #[test]
    fn verdict_fields_are_public() {
        let v = JudgeVerdict {
            score: 0.75,
            pass: true,
            reason: Some("looks right".into()),
            label: Some("equivalent".into()),
        };
        assert!((v.score - 0.75).abs() < f64::EPSILON);
        assert!(v.pass);
        assert_eq!(v.reason.as_deref(), Some("looks right"));
        assert_eq!(v.label.as_deref(), Some("equivalent"));
    }
}
