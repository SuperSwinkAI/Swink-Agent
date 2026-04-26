//! `TraceProvider` trait, error type, and in-memory OTel provider.
//!
//! This module defines the pull-side surface for external-trace ingestion
//! (spec 043 §FR-031): a `TraceProvider` fetches a `RawSession` from some
//! observability backend, and a `SessionMapper` then translates that
//! backend-specific payload into an internal `Invocation`.
//!
//! The always-available [`OtelInMemoryTraceProvider`] wraps
//! `opentelemetry-sdk`'s `InMemorySpanExporter` so tests and offline eval
//! pipelines can round-trip instrumented runs without provisioning a real
//! backend (R-005).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use opentelemetry_sdk::trace::{InMemorySpanExporter, SpanData};
use thiserror::Error;

// ─── RawSession ─────────────────────────────────────────────────────────────

/// Backend-agnostic payload returned by a [`TraceProvider`].
///
/// Session mappers (see `crate::trace::mapper`) translate this into an
/// internal `Invocation`. Currently the only shape we carry is a list of OTel
/// spans; concrete backend providers (Langfuse, OpenSearch, CloudWatch) will
/// add sibling variants in follow-up tasks T126–T129 without breaking
/// existing mappers (enum is `#[non_exhaustive]`).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum RawSession {
    /// OpenTelemetry spans making up one session. Spans are in arbitrary
    /// order; mappers MUST reconstruct parent/child relationships from
    /// `parent_span_id`.
    OtelSpans {
        /// Logical session identifier as requested from the provider.
        session_id: String,
        /// All spans that belong to the session. Guaranteed non-empty when
        /// returned by the provider (empty sessions surface as
        /// [`TraceProviderError::SessionNotFound`]).
        spans: Vec<SpanData>,
    },
}

impl RawSession {
    /// Session identifier this raw payload corresponds to.
    #[must_use]
    pub fn session_id(&self) -> &str {
        match self {
            Self::OtelSpans { session_id, .. } => session_id,
        }
    }
}

// ─── Error model ────────────────────────────────────────────────────────────

/// Errors a [`TraceProvider`] can surface while fetching a session.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum TraceProviderError {
    /// A trace provider for a backend whose cargo feature is disabled
    /// was requested at runtime (spec 043 US6 scenario 4 / T130).
    ///
    /// Compile-time access to `OpenSearchTraceProvider` /
    /// `CloudWatchTraceProvider` / `OtlpHttpTraceProvider` /
    /// `LangfuseTraceProvider` is already prevented by cargo features
    /// (each type is `#[cfg(feature = "…")]`). This variant covers the
    /// runtime configuration path — a builder that reads a backend name
    /// from a YAML config and has to refuse a name whose feature flag
    /// was not compiled in.
    #[error("trace backend `{backend}` is not available — enable the `{feature}` cargo feature")]
    FeatureDisabled {
        /// Name of the requested backend (e.g. `"opensearch"`).
        backend: String,
        /// Cargo feature the caller must enable.
        feature: String,
    },

    /// The requested session id is unknown to this backend.
    #[error("trace session `{session_id}` not found")]
    SessionNotFound {
        /// Session id the caller asked for.
        session_id: String,
    },

    /// The session exists but at least one of its spans has not yet ended
    /// (the root span is still "in progress"). Evaluators that compare
    /// complete runs MUST NOT consume a partial session.
    #[error("trace session `{session_id}` is still in progress ({open_spans} span(s) not ended)")]
    SessionInProgress {
        /// Session id the caller asked for.
        session_id: String,
        /// How many spans lack an `end_time` greater than `start_time`.
        open_spans: usize,
    },

    /// The backend failed in a way unrelated to session identity (network,
    /// auth, lock poisoning, …). String payload is free-form diagnostic.
    #[error("trace backend failure: {reason}")]
    BackendFailure {
        /// Human-readable diagnostic; not part of a stable contract.
        reason: String,
    },
}

// ─── Trait ──────────────────────────────────────────────────────────────────

/// Pull-side surface for external-trace ingestion (spec 043 FR-031).
///
/// Implementations SHOULD be cheap to clone (typically holding an `Arc` over
/// their backend handle) so they can be shared across concurrent eval runs.
pub trait TraceProvider: Send + Sync {
    /// Fetch the complete session identified by `session_id`.
    ///
    /// Implementations MUST fail with [`TraceProviderError::SessionInProgress`]
    /// rather than silently returning a partial trace — evaluators treat the
    /// returned `RawSession` as terminal.
    fn fetch_session<'a>(&'a self, session_id: &'a str) -> TraceProviderFuture<'a>;
}

/// Object-safe future returned by [`TraceProvider::fetch_session`].
pub type TraceProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<RawSession, TraceProviderError>> + Send + 'a>>;

// ─── OtelInMemoryTraceProvider ──────────────────────────────────────────────

/// Always-available provider backed by
/// [`opentelemetry_sdk::trace::InMemorySpanExporter`].
///
/// This provider is the round-trip fixture that backs SC-008 (OTel replay).
/// Spans are filtered by the `session.id` attribute; any span lacking one is
/// ignored. A span counts as "complete" when `end_time > start_time`.
///
/// The exporter instance is shared via `Arc`, so callers who also drive an
/// `SdkTracerProvider` just clone the exporter and hand one clone to the
/// tracer provider and one to this struct.
#[derive(Clone, Debug)]
pub struct OtelInMemoryTraceProvider {
    /// `InMemorySpanExporter` already holds its finished-span buffer behind
    /// an internal `Arc<Mutex<Vec<_>>>`, so cloning the exporter shares the
    /// same span storage with whatever `SdkTracerProvider` is writing into
    /// it. Keeping the field non-`Arc` avoids a redundant indirection.
    exporter: InMemorySpanExporter,
    /// Attribute key used to identify session membership. Defaults to
    /// `"session.id"` per OTel GenAI conventions; overridable for mappers
    /// that key on a different attribute (e.g. Langfuse `session_id`).
    session_attribute: Arc<str>,
}

impl OtelInMemoryTraceProvider {
    /// Wrap an existing `InMemorySpanExporter`. The exporter's finished-span
    /// buffer is shared, so spans recorded on `exporter` after construction
    /// are visible to subsequent `fetch_session` calls.
    #[must_use]
    pub fn new(exporter: InMemorySpanExporter) -> Self {
        Self {
            exporter,
            session_attribute: Arc::from("session.id"),
        }
    }

    /// Override the attribute key used to identify session membership.
    ///
    /// OTel GenAI conventions use `session.id`; OpenInference uses
    /// `openinference.session.id`; custom instrumentations may differ.
    #[must_use]
    pub fn with_session_attribute(mut self, key: impl Into<String>) -> Self {
        self.session_attribute = Arc::from(key.into());
        self
    }

    /// Attribute key this provider uses to filter spans by session.
    #[must_use]
    pub fn session_attribute(&self) -> &str {
        &self.session_attribute
    }

    /// Borrow the underlying exporter (escape hatch for tests that need to
    /// drive the exporter directly).
    #[must_use]
    pub fn exporter(&self) -> &InMemorySpanExporter {
        &self.exporter
    }
}

impl TraceProvider for OtelInMemoryTraceProvider {
    fn fetch_session<'a>(&'a self, session_id: &'a str) -> TraceProviderFuture<'a> {
        Box::pin(async move {
            let all = self.exporter.get_finished_spans().map_err(|err| {
                TraceProviderError::BackendFailure {
                    reason: format!("in-memory exporter lock: {err}"),
                }
            })?;

            let key = self.session_attribute.as_ref();
            let matching: Vec<SpanData> = all
                .into_iter()
                .filter(|span| {
                    span.attributes.iter().any(|kv| {
                        kv.key.as_str() == key && kv.value.as_str().as_ref() == session_id
                    })
                })
                .collect();

            if matching.is_empty() {
                return Err(TraceProviderError::SessionNotFound {
                    session_id: session_id.to_string(),
                });
            }

            let open_spans = matching
                .iter()
                .filter(|span| span.end_time <= span.start_time)
                .count();
            if open_spans > 0 {
                return Err(TraceProviderError::SessionInProgress {
                    session_id: session_id.to_string(),
                    open_spans,
                });
            }

            Ok(RawSession::OtelSpans {
                session_id: session_id.to_string(),
                spans: matching,
            })
        })
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{
        SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
    };
    use opentelemetry::{InstrumentationScope, KeyValue};
    use opentelemetry_sdk::trace::{SpanEvents, SpanLinks};
    use std::borrow::Cow;
    use std::time::{Duration, SystemTime};

    fn make_span(name: &str, attrs: Vec<KeyValue>, complete: bool) -> SpanData {
        let start = SystemTime::now();
        let end = if complete {
            start + Duration::from_millis(1)
        } else {
            start
        };
        SpanData {
            span_context: SpanContext::new(
                TraceId::from(1_u128),
                SpanId::from(1_u64),
                TraceFlags::default(),
                false,
                TraceState::default(),
            ),
            parent_span_id: SpanId::INVALID,
            parent_span_is_remote: false,
            span_kind: SpanKind::Internal,
            name: Cow::Owned(name.to_string()),
            start_time: start,
            end_time: end,
            attributes: attrs,
            dropped_attributes_count: 0,
            events: SpanEvents::default(),
            links: SpanLinks::default(),
            status: Status::Unset,
            instrumentation_scope: InstrumentationScope::builder("test").build(),
        }
    }

    #[test]
    fn raw_session_reports_session_id() {
        let s = RawSession::OtelSpans {
            session_id: "abc".into(),
            spans: vec![],
        };
        assert_eq!(s.session_id(), "abc");
    }

    #[test]
    fn trace_provider_error_display_includes_fields() {
        let err = TraceProviderError::SessionNotFound {
            session_id: "sid".into(),
        };
        assert!(format!("{err}").contains("sid"));
        let err = TraceProviderError::SessionInProgress {
            session_id: "sid".into(),
            open_spans: 2,
        };
        let rendered = format!("{err}");
        assert!(rendered.contains("sid"));
        assert!(rendered.contains('2'));
    }

    #[tokio::test]
    async fn fetch_session_not_found_when_no_spans_match() {
        let exporter = InMemorySpanExporter::default();
        let provider = OtelInMemoryTraceProvider::new(exporter);
        let err = provider
            .fetch_session("missing")
            .await
            .expect_err("empty exporter");
        assert!(matches!(err, TraceProviderError::SessionNotFound { .. }));
    }

    #[tokio::test]
    async fn fetch_session_uses_configured_attribute_key() {
        let exporter = InMemorySpanExporter::default();
        let provider =
            OtelInMemoryTraceProvider::new(exporter.clone()).with_session_attribute("custom.sid");
        assert_eq!(provider.session_attribute(), "custom.sid");

        // Writing into the exporter directly simulates a recorded session.
        use opentelemetry_sdk::trace::SpanExporter;
        let span = make_span("root", vec![KeyValue::new("custom.sid", "S1")], true);
        exporter.export(vec![span]).await.unwrap();

        let raw = provider.fetch_session("S1").await.unwrap();
        match raw {
            RawSession::OtelSpans { session_id, spans } => {
                assert_eq!(session_id, "S1");
                assert_eq!(spans.len(), 1);
            }
        }
    }
}
