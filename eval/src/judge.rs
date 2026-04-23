//! LLM-as-judge client trait and registry for semantic evaluators.
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

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

pub use crate::url_filter::{DefaultUrlFilter, UrlFilter};

/// Maximum retry attempts allowed for judge dispatch.
pub const MAX_RETRY_ATTEMPTS: u32 = 16;

/// Maximum supported judge batch size.
pub const MAX_BATCH_SIZE: usize = 128;

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

/// Retry configuration shared by judge-backed evaluators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Maximum number of attempts for one judge request.
    pub max_attempts: u32,
    /// Maximum backoff delay between attempts.
    pub max_delay: Duration,
    /// Whether retry jitter is enabled.
    pub jitter: bool,
}

impl RetryPolicy {
    /// Create a retry policy with explicit values.
    #[must_use]
    pub const fn new(max_attempts: u32, max_delay: Duration, jitter: bool) -> Self {
        Self {
            max_attempts,
            max_delay,
            jitter,
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 6,
            max_delay: Duration::from_secs(240),
            jitter: true,
        }
    }
}

/// Configuration binding a judge client to a required model identifier.
///
/// This registry intentionally does not provide a default model. Provider model
/// lineups change over time, so callers must make model identity explicit at
/// construction to preserve score comparability.
pub struct JudgeRegistry {
    client: Arc<dyn JudgeClient>,
    model_id: String,
    retry_policy: RetryPolicy,
    batch_size: usize,
    url_filter: Arc<dyn UrlFilter>,
}

impl std::fmt::Debug for JudgeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JudgeRegistry")
            .field("model_id", &self.model_id)
            .field("retry_policy", &self.retry_policy)
            .field("batch_size", &self.batch_size)
            .finish_non_exhaustive()
    }
}

impl JudgeRegistry {
    /// Start building a judge registry for an explicit model identifier.
    #[must_use]
    pub fn builder(
        client: Arc<dyn JudgeClient>,
        model_id: impl Into<String>,
    ) -> JudgeRegistryBuilder {
        JudgeRegistryBuilder {
            client,
            model_id: model_id.into(),
            retry_policy: RetryPolicy::default(),
            batch_size: 1,
            url_filter: Arc::new(DefaultUrlFilter),
        }
    }

    /// Borrow the configured judge client.
    #[must_use]
    pub fn client(&self) -> &Arc<dyn JudgeClient> {
        &self.client
    }

    /// Return the explicit judge model identifier.
    #[must_use]
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    /// Return the retry policy used by judge-backed evaluators.
    #[must_use]
    pub const fn retry_policy(&self) -> &RetryPolicy {
        &self.retry_policy
    }

    /// Return the bounded batch size for judge dispatch.
    #[must_use]
    pub const fn batch_size(&self) -> usize {
        self.batch_size
    }

    /// Borrow the URL filter used when materializing judge attachments.
    #[must_use]
    pub fn url_filter(&self) -> &Arc<dyn UrlFilter> {
        &self.url_filter
    }
}

/// Builder for [`JudgeRegistry`].
pub struct JudgeRegistryBuilder {
    client: Arc<dyn JudgeClient>,
    model_id: String,
    retry_policy: RetryPolicy,
    batch_size: usize,
    url_filter: Arc<dyn UrlFilter>,
}

impl JudgeRegistryBuilder {
    /// Override retry behavior.
    #[must_use]
    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    /// Override the bounded judge batch size.
    #[must_use]
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Override the URL filter used by attachment materialization.
    #[must_use]
    pub fn with_url_filter(mut self, url_filter: Arc<dyn UrlFilter>) -> Self {
        self.url_filter = url_filter;
        self
    }

    /// Validate and construct the registry.
    pub fn build(self) -> Result<JudgeRegistry, JudgeRegistryError> {
        let model_id = self.model_id.trim().to_string();
        if model_id.is_empty() {
            return Err(JudgeRegistryError::MissingModelId);
        }
        if !(1..=MAX_BATCH_SIZE).contains(&self.batch_size) {
            return Err(JudgeRegistryError::InvalidBatchSize {
                batch_size: self.batch_size,
            });
        }
        if self.retry_policy.max_attempts > MAX_RETRY_ATTEMPTS {
            return Err(JudgeRegistryError::InvalidRetryPolicy {
                reason: format!(
                    "max_attempts must be <= {MAX_RETRY_ATTEMPTS}, got {}",
                    self.retry_policy.max_attempts
                ),
            });
        }
        if self.retry_policy.max_attempts == 0 {
            return Err(JudgeRegistryError::InvalidRetryPolicy {
                reason: "max_attempts must be greater than 0".to_string(),
            });
        }

        Ok(JudgeRegistry {
            client: self.client,
            model_id,
            retry_policy: self.retry_policy,
            batch_size: self.batch_size,
            url_filter: self.url_filter,
        })
    }
}

/// Errors returned while constructing a [`JudgeRegistry`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum JudgeRegistryError {
    /// No explicit model identifier was provided.
    #[error("judge registry requires an explicit model_id")]
    MissingModelId,
    /// Batch size was outside the supported `[1, 128]` range.
    #[error("judge batch_size must be in 1..={MAX_BATCH_SIZE}, got {batch_size}")]
    InvalidBatchSize { batch_size: usize },
    /// Retry policy is outside the supported bounds.
    #[error("invalid judge retry policy: {reason}")]
    InvalidRetryPolicy { reason: String },
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
