//! `OpenSearchTraceProvider` — pulls a session from an OpenSearch index
//! that stores OpenTelemetry spans and translates the hits into a
//! [`RawSession`] (spec 043 T128).
//!
//! Assumes a document-per-span layout where each hit carries at least:
//!
//! * `name`  — span name string.
//! * `start_time` / `end_time` — RFC 3339 timestamps (missing `end_time`
//!   means the span is still open and surfaces as
//!   [`TraceProviderError::SessionInProgress`]).
//! * A `span_id` / `parent_span_id` pair — opaque strings (hashed
//!   deterministically into OTel ids, matching `langfuse.rs`).
//! * An `attributes` object — every non-null scalar becomes an OTel
//!   `KeyValue`; the session-attribute value identifies membership.
//!
//! Transport: `POST {base_url}/{index}/_search` with a
//! match-by-session-attribute filter. `OPENSEARCH_API_KEY` authentication
//! is optional; when present it is forwarded as a Bearer token, matching
//! OpenSearch's HTTP API conventions. Other transports (AWS SigV4, Basic
//! auth, mTLS) are expected to be handled by the caller's reverse proxy
//! or a future builder method; this provider intentionally keeps a small
//! surface.

#![cfg(feature = "trace-opensearch")]

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{InstrumentationScope, KeyValue, Value};
use opentelemetry_sdk::trace::{SpanData, SpanEvents, SpanLinks};
use reqwest::Client;
use serde::Deserialize;

use crate::trace::provider::{RawSession, TraceProvider, TraceProviderError};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_SESSION_ATTRIBUTE: &str = "session.id";
const DEFAULT_HITS_LIMIT: usize = 10_000;

/// Trace provider backed by OpenSearch's `_search` HTTP endpoint.
///
/// Construct with a base URL + index; supply an optional bearer token
/// via [`Self::with_bearer`]. All spans sharing a `session.id`
/// attribute value are returned as one [`RawSession::OtelSpans`].
#[derive(Clone, Debug)]
pub struct OpenSearchTraceProvider {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: Client,
    base_url: String,
    index: String,
    bearer: Option<String>,
    session_attribute: String,
    hits_limit: usize,
}

impl OpenSearchTraceProvider {
    /// Build a provider targeting `base_url` against the given `index`.
    ///
    /// Trailing slashes on `base_url` are normalized away. The
    /// session-attribute defaults to `session.id` and the result
    /// hits-limit defaults to 10,000 (OpenSearch's default `size` cap —
    /// use [`Self::with_hits_limit`] to raise it if your sessions can
    /// exceed that).
    ///
    /// # Errors
    /// Surfaces `reqwest::Client::builder()` construction failures as
    /// [`TraceProviderError::BackendFailure`].
    pub fn new(
        base_url: impl Into<String>,
        index: impl Into<String>,
    ) -> Result<Self, TraceProviderError> {
        let http = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|err| TraceProviderError::BackendFailure {
                reason: format!("reqwest client build: {err}"),
            })?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                base_url: base_url.into().trim_end_matches('/').to_string(),
                index: index.into(),
                bearer: None,
                session_attribute: DEFAULT_SESSION_ATTRIBUTE.to_string(),
                hits_limit: DEFAULT_HITS_LIMIT,
            }),
        })
    }

    /// Attach a bearer token forwarded as `Authorization: Bearer <token>`.
    #[must_use]
    pub fn with_bearer(mut self, token: impl Into<String>) -> Self {
        Arc::make_mut(&mut self.inner).bearer = Some(token.into());
        self
    }

    /// Override the attribute key used to match session membership.
    #[must_use]
    pub fn with_session_attribute(mut self, attribute: impl Into<String>) -> Self {
        Arc::make_mut(&mut self.inner).session_attribute = attribute.into();
        self
    }

    /// Override the `size` parameter passed on the search body.
    #[must_use]
    pub fn with_hits_limit(mut self, limit: usize) -> Self {
        Arc::make_mut(&mut self.inner).hits_limit = limit;
        self
    }

    /// Base URL the provider targets.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    /// Index the provider queries.
    #[must_use]
    pub fn index(&self) -> &str {
        &self.inner.index
    }

    /// Attribute key matched on `session_id`.
    #[must_use]
    pub fn session_attribute(&self) -> &str {
        &self.inner.session_attribute
    }
}

// `Arc::make_mut` clones behind the scenes, so `Inner` needs Clone.
impl Clone for Inner {
    fn clone(&self) -> Self {
        Self {
            http: self.http.clone(),
            base_url: self.base_url.clone(),
            index: self.index.clone(),
            bearer: self.bearer.clone(),
            session_attribute: self.session_attribute.clone(),
            hits_limit: self.hits_limit,
        }
    }
}

#[async_trait]
impl TraceProvider for OpenSearchTraceProvider {
    async fn fetch_session(&self, session_id: &str) -> Result<RawSession, TraceProviderError> {
        let url = format!("{}/{}/_search", self.inner.base_url, self.inner.index);
        let body = serde_json::json!({
            "size": self.inner.hits_limit,
            "query": {
                "term": {
                    format!("attributes.{}.keyword", self.inner.session_attribute): session_id,
                }
            }
        });
        let mut req = self.inner.http.post(&url).json(&body);
        if let Some(token) = &self.inner.bearer {
            req = req.bearer_auth(token);
        }
        let resp = req
            .send()
            .await
            .map_err(|err| TraceProviderError::BackendFailure {
                reason: format!("opensearch POST {url}: {err}"),
            })?;

        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(TraceProviderError::SessionNotFound {
                session_id: session_id.to_string(),
            });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(TraceProviderError::BackendFailure {
                reason: format!("opensearch http {}: {}", status.as_u16(), truncate(&body)),
            });
        }

        let body: SearchBody =
            resp.json()
                .await
                .map_err(|err| TraceProviderError::BackendFailure {
                    reason: format!("opensearch body parse: {err}"),
                })?;
        let hits = body.hits.hits;
        if hits.is_empty() {
            return Err(TraceProviderError::SessionNotFound {
                session_id: session_id.to_string(),
            });
        }

        let open: usize = hits.iter().filter(|h| h.source.end_time.is_none()).count();
        if open > 0 {
            return Err(TraceProviderError::SessionInProgress {
                session_id: session_id.to_string(),
                open_spans: open,
            });
        }

        let spans = hits
            .into_iter()
            .map(|h| hit_to_span_data(h.source, session_id, &self.inner.session_attribute))
            .collect();

        Ok(RawSession::OtelSpans {
            session_id: session_id.to_string(),
            spans,
        })
    }
}

fn truncate(s: &str) -> String {
    const LIMIT: usize = 512;
    if s.len() <= LIMIT {
        s.to_string()
    } else {
        let mut out = s[..LIMIT].to_string();
        out.push_str("...<truncated>");
        out
    }
}

// ─── Wire types ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SearchBody {
    hits: Hits,
}

#[derive(Debug, Deserialize)]
struct Hits {
    hits: Vec<Hit>,
}

#[derive(Debug, Deserialize)]
struct Hit {
    #[serde(rename = "_source")]
    source: SourceDoc,
}

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

fn hit_to_span_data(doc: SourceDoc, session_id: &str, session_attr: &str) -> SpanData {
    let name = doc.name.unwrap_or_else(|| "opensearch.span".to_string());

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

    // Seed attributes with the session id so downstream mappers recognise it.
    let mut attributes: Vec<KeyValue> = Vec::new();
    attributes.push(KeyValue::new(
        session_attr.to_string(),
        session_id.to_string(),
    ));
    if let Some(attrs) = doc.attributes
        && let Some(map) = attrs.as_object()
    {
        for (k, v) in map {
            // Skip the session attribute we already inserted above (avoid dupes).
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
        instrumentation_scope: InstrumentationScope::builder("opensearch").build(),
    }
}

/// Minimal RFC 3339 parser shared with `langfuse.rs` (duplicated for
/// crate-local availability without exposing a new public helper).
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

    #[test]
    fn hash_to_span_id_is_deterministic() {
        let a = hash_to_span_id("span-1");
        let b = hash_to_span_id("span-1");
        assert_eq!(a, b);
        assert_ne!(hash_to_span_id("span-1"), hash_to_span_id("span-2"));
    }

    #[test]
    fn parse_rfc3339_handles_utc_suffix() {
        let t = parse_rfc3339("2026-04-23T10:00:00Z").expect("rfc3339");
        let later = parse_rfc3339("2026-04-23T10:00:01Z").expect("rfc3339");
        assert!(later > t);
    }
}
