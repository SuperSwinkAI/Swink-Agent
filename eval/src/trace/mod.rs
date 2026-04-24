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

pub mod extractor;
#[cfg(feature = "trace-langfuse")]
pub mod langfuse;
pub mod mapper;
#[cfg(feature = "trace-otlp")]
pub mod otlp;
pub mod provider;

pub use extractor::{EvaluationLevel, ExtractedInput, TraceExtractor};
#[cfg(feature = "trace-langfuse")]
pub use langfuse::LangfuseTraceProvider;
pub use mapper::{
    GenAIAttributeTable, GenAIConventionVersion, LangChainSessionMapper, MappingError,
    OpenInferenceSessionMapper, OtelGenAiSessionMapper, SessionMapper,
};
#[cfg(feature = "trace-otlp")]
pub use otlp::OtlpHttpTraceProvider;
pub use provider::{OtelInMemoryTraceProvider, RawSession, TraceProvider, TraceProviderError};
