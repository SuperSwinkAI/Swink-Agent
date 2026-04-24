//! `CloudWatchTraceProvider` — turns CloudWatch Logs events into a
//! [`RawSession`] (spec 043 T129).
//!
//! CloudWatch Logs is accessed via AWS APIs that require SigV4 signing
//! and the `aws-sdk-cloudwatchlogs` crate, which we deliberately do not
//! pull into the eval workspace (SC-009 keeps the default dep graph
//! minimal and Ring/tonic pulls already dominate `all-features`). The
//! provider therefore consumes a caller-supplied
//! [`CloudWatchLogsFetcher`] trait implementation — downstream users
//! plug in their own AWS SDK or signing layer, and this crate stays
//! dependency-light.
//!
//! Each fetched event is expected to be a JSON document representing one
//! OTel span (the shape the CloudWatch Logs → OpenTelemetry Collector
//! pipeline emits). Missing `end_time` surfaces as
//! [`TraceProviderError::SessionInProgress`]. All parsing logic mirrors
//! [`crate::trace::opensearch`] so replay semantics are stable across
//! backends.

#![cfg(feature = "trace-cloudwatch")]

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{InstrumentationScope, KeyValue, Value};
use opentelemetry_sdk::trace::{SpanData, SpanEvents, SpanLinks};
use serde::Deserialize;

use crate::trace::provider::{RawSession, TraceProvider, TraceProviderError};

const DEFAULT_SESSION_ATTRIBUTE: &str = "session.id";

/// Caller-supplied fetcher that drops a thin AWS-SDK layer under the
/// provider (spec 043 T129).
///
/// Implementations issue a `StartQuery` + `GetQueryResults` pair
/// against CloudWatch Logs Insights (or any equivalent mechanism) and
/// return one JSON document per span. The provider is agnostic to how
/// the events were filtered as long as every returned event shares the
/// requested `session_id`.
///
/// Implementations SHOULD surface transport / auth failures as
/// [`TraceProviderError::BackendFailure`]; empty results SHOULD return
/// [`TraceProviderError::SessionNotFound`] directly or an empty vector
/// (the provider converts empty to `SessionNotFound`).
#[async_trait]
pub trait CloudWatchLogsFetcher: Send + Sync {
    /// Fetch the raw JSON documents for `session_id`.
    ///
    /// The documents themselves are not yet OTel spans — the provider
    /// parses them into [`SpanData`].
    async fn fetch_events(
        &self,
        session_id: &str,
    ) -> Result<Vec<serde_json::Value>, TraceProviderError>;
}

/// Trace provider backed by a caller-supplied [`CloudWatchLogsFetcher`].
///
/// Swap the fetcher in tests for an in-memory implementation; supply a
/// SigV4-aware fetcher in production.
#[derive(Clone)]
pub struct CloudWatchTraceProvider {
    fetcher: Arc<dyn CloudWatchLogsFetcher>,
    session_attribute: Arc<str>,
}

impl std::fmt::Debug for CloudWatchTraceProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudWatchTraceProvider")
            .field("session_attribute", &self.session_attribute)
            .finish_non_exhaustive()
    }
}

impl CloudWatchTraceProvider {
    /// Build a provider backed by the supplied fetcher.
    #[must_use]
    pub fn new(fetcher: Arc<dyn CloudWatchLogsFetcher>) -> Self {
        Self {
            fetcher,
            session_attribute: Arc::from(DEFAULT_SESSION_ATTRIBUTE),
        }
    }

    /// Override the attribute key treated as session identity in the
    /// emitted spans.
    #[must_use]
    pub fn with_session_attribute(mut self, attribute: impl Into<String>) -> Self {
        self.session_attribute = Arc::from(attribute.into());
        self
    }

    /// Attribute key this provider stamps on each span.
    #[must_use]
    pub fn session_attribute(&self) -> &str {
        &self.session_attribute
    }
}

#[async_trait]
impl TraceProvider for CloudWatchTraceProvider {
    async fn fetch_session(&self, session_id: &str) -> Result<RawSession, TraceProviderError> {
        let events = self.fetcher.fetch_events(session_id).await?;
        if events.is_empty() {
            return Err(TraceProviderError::SessionNotFound {
                session_id: session_id.to_string(),
            });
        }

        let mut docs: Vec<SourceDoc> = Vec::with_capacity(events.len());
        for value in events {
            let doc: SourceDoc = serde_json::from_value(value).map_err(|err| {
                TraceProviderError::BackendFailure {
                    reason: format!("cloudwatch event parse: {err}"),
                }
            })?;
            docs.push(doc);
        }

        let open = docs.iter().filter(|d| d.end_time.is_none()).count();
        if open > 0 {
            return Err(TraceProviderError::SessionInProgress {
                session_id: session_id.to_string(),
                open_spans: open,
            });
        }

        let spans = docs
            .into_iter()
            .map(|d| doc_to_span_data(d, session_id, &self.session_attribute))
            .collect();

        Ok(RawSession::OtelSpans {
            session_id: session_id.to_string(),
            spans,
        })
    }
}

// ─── Wire / parsing types ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SourceDoc {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    span_id: Option<String>,
    #[serde(default)]
    parent_span_id: Option<String>,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    end_time: Option<String>,
    #[serde(default)]
    attributes: Option<serde_json::Value>,
    #[serde(default)]
    kind: Option<String>,
}

fn doc_to_span_data(doc: SourceDoc, session_id: &str, session_attr: &str) -> SpanData {
    let name = doc.name.unwrap_or_else(|| "cloudwatch.span".to_string());
    let start = doc
        .start_time
        .as_deref()
        .and_then(parse_rfc3339)
        .unwrap_or_else(SystemTime::now);
    let end = doc
        .end_time
        .as_deref()
        .and_then(parse_rfc3339)
        .unwrap_or(start + Duration::from_millis(1));

    let span_id = doc
        .span_id
        .as_deref()
        .map_or(SpanId::INVALID, hash_to_span_id);
    let parent_span_id = doc
        .parent_span_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .map_or(SpanId::INVALID, hash_to_span_id);
    let trace_id = hash_to_trace_id(session_id);

    let span_context = SpanContext::new(
        trace_id,
        span_id,
        TraceFlags::default(),
        false,
        TraceState::default(),
    );
    let span_kind = match doc.kind.as_deref() {
        Some("client" | "CLIENT") => SpanKind::Client,
        Some("server" | "SERVER") => SpanKind::Server,
        Some("producer" | "PRODUCER") => SpanKind::Producer,
        Some("consumer" | "CONSUMER") => SpanKind::Consumer,
        _ => SpanKind::Internal,
    };

    let mut attributes: Vec<KeyValue> = Vec::new();
    attributes.push(KeyValue::new(
        session_attr.to_string(),
        session_id.to_string(),
    ));
    if let Some(attrs) = doc.attributes
        && let Some(map) = attrs.as_object()
    {
        for (k, v) in map {
            if k == session_attr {
                continue;
            }
            if let Some(value) = json_to_otel_value(v) {
                attributes.push(KeyValue::new(k.clone(), value));
            }
        }
    }

    SpanData {
        span_context,
        parent_span_id,
        parent_span_is_remote: false,
        span_kind,
        name: Cow::Owned(name),
        start_time: start,
        end_time: end,
        attributes,
        dropped_attributes_count: 0,
        events: SpanEvents::default(),
        links: SpanLinks::default(),
        status: Status::Unset,
        instrumentation_scope: InstrumentationScope::builder("cloudwatch").build(),
    }
}

fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    let (date, rest) = s.split_once('T')?;
    let mut date_parts = date.splitn(3, '-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;

    let (time_part, tz_part) = split_tz(rest)?;
    let (hms, frac_nanos) = match time_part.split_once('.') {
        Some((hms, frac)) => (hms, parse_fraction_to_nanos(frac)),
        None => (time_part, 0),
    };
    let mut hms_parts = hms.splitn(3, ':');
    let hour: u32 = hms_parts.next()?.parse().ok()?;
    let minute: u32 = hms_parts.next()?.parse().ok()?;
    let second: u32 = hms_parts.next()?.parse().ok()?;

    let tz_offset_seconds = parse_tz_offset(tz_part)?;
    let days = days_from_civil(year, month, day);
    let secs_in_day =
        i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second) - tz_offset_seconds;
    let total_seconds = days * 86_400 + secs_in_day;
    if total_seconds < 0 {
        return None;
    }
    #[allow(clippy::cast_sign_loss)]
    let unix_seconds = total_seconds as u64;
    Some(UNIX_EPOCH + Duration::new(unix_seconds, frac_nanos))
}

fn split_tz(rest: &str) -> Option<(&str, &str)> {
    if let Some(idx) = rest.find(['Z', 'z']) {
        return Some((&rest[..idx], "Z"));
    }
    let bytes = rest.as_bytes();
    for (i, &b) in bytes.iter().enumerate().rev() {
        if (b == b'+' || b == b'-') && i > 0 {
            return Some((&rest[..i], &rest[i..]));
        }
    }
    None
}

fn parse_fraction_to_nanos(frac: &str) -> u32 {
    let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return 0;
    }
    let mut padded = digits;
    while padded.len() < 9 {
        padded.push('0');
    }
    padded[..9].parse::<u32>().unwrap_or(0)
}

fn parse_tz_offset(tz: &str) -> Option<i64> {
    if tz == "Z" || tz == "z" {
        return Some(0);
    }
    let sign = match tz.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let body = &tz[1..];
    let (h, m) = body.split_once(':').unwrap_or((body, "0"));
    let h: i64 = h.parse().ok()?;
    let m: i64 = m.parse().ok()?;
    Some(sign * (h * 3600 + m * 60))
}

#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let m_i = i64::from(m);
    let d_i = i64::from(d);
    let y = if m_i <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m_i > 2 { m_i - 3 } else { m_i + 9 }) + 2) / 5 + d_i - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn json_to_otel_value(v: &serde_json::Value) -> Option<Value> {
    Some(match v {
        serde_json::Value::String(s) => Value::String(s.clone().into()),
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::I64(i)
            } else if let Some(u) = n.as_u64() {
                Value::I64(i64::try_from(u).unwrap_or(i64::MAX))
            } else if let Some(f) = n.as_f64() {
                Value::F64(f)
            } else {
                return None;
            }
        }
        serde_json::Value::Null => return None,
        other => Value::String(other.to_string().into()),
    })
}

fn hash_to_span_id(s: &str) -> SpanId {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(s.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    SpanId::from_bytes(bytes)
}

fn hash_to_trace_id(s: &str) -> TraceId {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(s.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    TraceId::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StaticFetcher(Vec<serde_json::Value>);

    #[async_trait]
    impl CloudWatchLogsFetcher for StaticFetcher {
        async fn fetch_events(
            &self,
            _session_id: &str,
        ) -> Result<Vec<serde_json::Value>, TraceProviderError> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn session_not_found_when_fetcher_returns_empty() {
        let provider = CloudWatchTraceProvider::new(Arc::new(StaticFetcher(vec![])) as Arc<_>);
        let err = provider.fetch_session("sid").await.expect_err("empty");
        assert!(matches!(err, TraceProviderError::SessionNotFound { .. }));
    }

    #[tokio::test]
    async fn session_in_progress_when_any_span_has_no_end_time() {
        let events = vec![
            serde_json::json!({
                "name": "root",
                "span_id": "s1",
                "start_time": "2026-04-23T10:00:00Z",
                "end_time": "2026-04-23T10:00:01Z",
            }),
            serde_json::json!({
                "name": "open",
                "span_id": "s2",
                "start_time": "2026-04-23T10:00:02Z",
            }),
        ];
        let provider = CloudWatchTraceProvider::new(Arc::new(StaticFetcher(events)) as Arc<_>);
        let err = provider.fetch_session("sid").await.expect_err("open span");
        match err {
            TraceProviderError::SessionInProgress { open_spans, .. } => {
                assert_eq!(open_spans, 1);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn happy_path_emits_spans_with_session_attribute() {
        let events = vec![serde_json::json!({
            "name": "root",
            "span_id": "s1",
            "start_time": "2026-04-23T10:00:00Z",
            "end_time": "2026-04-23T10:00:01Z",
            "kind": "client",
            "attributes": {
                "model": "gpt-4",
                "token.count": 42,
            },
        })];
        let provider = CloudWatchTraceProvider::new(Arc::new(StaticFetcher(events)) as Arc<_>);
        let raw = provider.fetch_session("sid").await.expect("ok");
        match raw {
            RawSession::OtelSpans { session_id, spans } => {
                assert_eq!(session_id, "sid");
                assert_eq!(spans.len(), 1);
                let attrs: Vec<&str> = spans[0]
                    .attributes
                    .iter()
                    .map(|kv| kv.key.as_str())
                    .collect();
                assert!(attrs.contains(&"session.id"));
                assert!(attrs.contains(&"model"));
                assert!(attrs.contains(&"token.count"));
            }
        }
    }
}
