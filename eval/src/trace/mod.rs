//! Trace ingestion providers, mappers, and extractors (spec 043 US6).
//!
//! Behind the `trace-ingest` feature. See:
//! * [`provider`] — pull-side abstraction (`TraceProvider`,
//!   `TraceProviderError`, `RawSession`, `OtelInMemoryTraceProvider`).
//! * [`mapper`] — convert a `RawSession` into an `Invocation` per a
//!   semantic convention (`SessionMapper`, `OpenInferenceSessionMapper`,
//!   `LangChainSessionMapper`, `OtelGenAiSessionMapper` +
//!   `GenAIConventionVersion`).
//! * [`extractor`] — produce evaluator inputs at a requested
//!   [`extractor::EvaluationLevel`] via the [`extractor::TraceExtractor`]
//!   trait.
//!
//! # Feature-gated backends (T130)
//!
//! Each concrete backend provider lives behind its own cargo feature:
//!
//! | Backend      | Cargo feature          | Module                  |
//! | ------------ | ---------------------- | ----------------------- |
//! | OTLP HTTP    | `trace-otlp`           | [`otlp`]                |
//! | Langfuse     | `trace-langfuse`       | [`langfuse`]            |
//! | OpenSearch   | `trace-opensearch`     | [`opensearch`]          |
//! | CloudWatch   | `trace-cloudwatch`     | [`cloudwatch`]          |
//!
//! Accessing any of these types without its feature enabled is a
//! compile-time error — `cfg` gating removes the type entirely. For
//! runtime configurations (e.g. "user picks a backend by name from a
//! YAML file") use [`TraceProviderError::FeatureDisabled`] to surface a
//! clear runtime error instead of silently falling back:
//!
//! ```rust
//! use swink_agent_eval::trace::TraceProviderError;
//!
//! fn pick_backend(name: &str) -> Result<(), TraceProviderError> {
//!     match name {
//!         "otel-in-memory" => Ok(()),
//!         #[cfg(feature = "trace-opensearch")]
//!         "opensearch" => Ok(()),
//!         "opensearch" => Err(TraceProviderError::FeatureDisabled {
//!             backend: "opensearch".into(),
//!             feature: "trace-opensearch".into(),
//!         }),
//!         _ => Err(TraceProviderError::BackendFailure {
//!             reason: format!("unknown backend `{name}`"),
//!         }),
//!     }
//! }
//! # let err = pick_backend("opensearch").unwrap_or(());
//! ```

#[cfg(feature = "trace-cloudwatch")]
pub mod cloudwatch;
pub mod extractor;
#[cfg(feature = "trace-langfuse")]
pub mod langfuse;
pub mod mapper;
#[cfg(feature = "trace-opensearch")]
pub mod opensearch;
#[cfg(feature = "trace-otlp")]
pub mod otlp;
pub mod provider;

#[cfg(feature = "trace-cloudwatch")]
pub use cloudwatch::{CloudWatchLogsFetcher, CloudWatchTraceProvider};
pub use extractor::{
    EvaluationLevel, ExtractedInput, GraphExtractor, SwarmExtractor, ToolLevelExtractor,
    TraceExtractor,
};
#[cfg(feature = "trace-langfuse")]
pub use langfuse::LangfuseTraceProvider;
pub use mapper::{
    GenAIAttributeTable, GenAIConventionVersion, LangChainSessionMapper, MappingError,
    OpenInferenceSessionMapper, OtelGenAiSessionMapper, SessionMapper,
};
#[cfg(feature = "trace-opensearch")]
pub use opensearch::OpenSearchTraceProvider;
#[cfg(feature = "trace-otlp")]
pub use otlp::OtlpHttpTraceProvider;
pub use provider::{OtelInMemoryTraceProvider, RawSession, TraceProvider, TraceProviderError};
