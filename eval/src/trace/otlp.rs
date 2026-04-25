//! `OtlpHttpTraceProvider` — OTLP-HTTP *pull* adapter (spec 043 T126).
//!
//! OTLP is, in the wild, overwhelmingly a *push* protocol: instrumented
//! applications `POST` batches of spans to a collector. For offline
//! evaluation (FR-031, R-005), we need the opposite direction: given a
//! session id, fetch every span that belongs to it from whatever backend
//! is storing them. A growing number of OTLP-aware backends (Tempo,
//! Grafana OTLP gateway, custom collectors) expose an HTTP JSON query
//! surface that returns stored spans in the standard OTLP-JSON shape
//! (`resourceSpans[].scopeSpans[].spans[]`). This provider targets that
//! surface — it is NOT an exporter, and it does not speak gRPC/protobuf.
//!
//! The wire format consumed here is the OTLP-HTTP+JSON body as defined by
//! `opentelemetry-proto` (encoded on the wire as UTF-8 JSON). We only
//! need a thin deserializer for it; pulling in `opentelemetry-proto` for
//! one struct would add a default-feature-visible build step (SC-009), so
//! we define the minimal serde-ready shape locally. Only the fields
//! mappers actually read (span id, parent span id, timing, attributes,
//! name) are parsed.
//!
//! Filtering is done on the client side: the provider queries the
//! backend for a session id, then retains only spans carrying the
//! configured session attribute. Any span missing `end_time > start_time`
//! (the OTLP wire uses unix-nanos; we compare numerically) causes the
//! provider to surface [`TraceProviderError::SessionInProgress`] — the
//! same contract `OtelInMemoryTraceProvider` honors.

#![cfg(feature = "trace-otlp")]

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{InstrumentationScope, KeyValue, Value};
use opentelemetry_sdk::trace::{SpanData, SpanEvents, SpanLinks};
use reqwest::Client;
use serde::Deserialize;

use crate::trace::provider::{RawSession, TraceProvider, TraceProviderError};

const DEFAULT_SESSION_ATTRIBUTE: &str = "session.id";
const DEFAULT_SESSION_QUERY_PARAM: &str = "session.id";
const DEFAULT_PATH: &str = "/v1/traces";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Pull-mode trace provider that fetches stored spans from an OTLP-HTTP
/// compatible query endpoint (spec 043 FR-031, T126).
///
/// Configure by pointing [`OtlpHttpTraceProvider::new`] at the backend
/// base URL (e.g. `"http://localhost:4318"`). `fetch_session` issues a
/// `GET {base_url}{path}?{session_query_param}={session_id}` request;
/// the response body is parsed as OTLP-JSON and filtered to spans whose
/// configured session attribute equals `session_id`.
#[derive(Clone, Debug)]
pub struct OtlpHttpTraceProvider {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: Client,
    base_url: String,
    path: String,
    session_attribute: String,
    session_query_param: String,
    bearer_token: Option<String>,
}

impl OtlpHttpTraceProvider {
    /// Build a provider pointed at `base_url`.
    ///
    /// Trailing slashes on `base_url` are normalized away.
    ///
    /// # Errors
    /// Propagates any `reqwest::Client::builder()` construction failure
    /// (e.g. TLS backend initialization) as
    /// [`TraceProviderError::BackendFailure`].
    pub fn new(base_url: impl Into<String>) -> Result<Self, TraceProviderError> {
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
                path: DEFAULT_PATH.to_string(),
                session_attribute: DEFAULT_SESSION_ATTRIBUTE.to_string(),
                session_query_param: DEFAULT_SESSION_QUERY_PARAM.to_string(),
                bearer_token: None,
            }),
        })
    }

    /// Override the endpoint path (default `/v1/traces`).
    #[must_use]
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        Arc::make_mut(&mut self.inner).path = path.into();
        self
    }

    /// Override the attribute key used to recognize session membership on
    /// returned spans (default `session.id`).
    #[must_use]
    pub fn with_session_attribute(mut self, key: impl Into<String>) -> Self {
        Arc::make_mut(&mut self.inner).session_attribute = key.into();
        self
    }

    /// Override the query-parameter name used to convey the session id to
    /// the backend (default `session.id`). Some backends key on
    /// `session_id` or a custom name.
    #[must_use]
    pub fn with_session_query_param(mut self, name: impl Into<String>) -> Self {
        Arc::make_mut(&mut self.inner).session_query_param = name.into();
        self
    }

    /// Attach a bearer token. Sent as `Authorization: Bearer <token>`
    /// on every fetch.
    #[must_use]
    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        Arc::make_mut(&mut self.inner).bearer_token = Some(token.into());
        self
    }

    /// Base URL the provider targets (debug/introspection helper).
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    /// Session attribute key this provider filters on.
    #[must_use]
    pub fn session_attribute(&self) -> &str {
        &self.inner.session_attribute
    }
}

// `Inner` holds no `reqwest`-owned mutex and each field is trivially clone.
// Declaring Clone manually avoids the "cannot be automatically derived"
// friction of having `Client` in the struct and keeps `Arc::make_mut`
// happy in the builder methods above.
impl Clone for Inner {
    fn clone(&self) -> Self {
        Self {
            http: self.http.clone(),
            base_url: self.base_url.clone(),
            path: self.path.clone(),
            session_attribute: self.session_attribute.clone(),
            session_query_param: self.session_query_param.clone(),
            bearer_token: self.bearer_token.clone(),
        }
    }
}

impl TraceProvider for OtlpHttpTraceProvider {
    fn fetch_session<'a>(
        &'a self,
        session_id: &'a str,
    ) -> crate::trace::provider::TraceProviderFuture<'a> {
        Box::pin(async move {
            let encoded: String = url::form_urlencoded::Serializer::new(String::new())
                .append_pair(&self.inner.session_query_param, session_id)
                .finish();
            let full_url = format!("{}{}?{}", self.inner.base_url, self.inner.path, encoded);

            let mut req = self.inner.http.get(&full_url);
            if let Some(token) = &self.inner.bearer_token {
                req = req.bearer_auth(token);
            }

            let resp = req
                .send()
                .await
                .map_err(|err| TraceProviderError::BackendFailure {
                    reason: format!("otlp GET {full_url}: {err}"),
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
                    reason: format!("otlp http {}: {}", status.as_u16(), truncate(&body)),
                });
            }

            let body: OtlpTraceResponse =
                resp.json()
                    .await
                    .map_err(|err| TraceProviderError::BackendFailure {
                        reason: format!("otlp body parse: {err}"),
                    })?;

            let all_spans = body.into_span_data();

            let key = self.inner.session_attribute.as_str();
            let matching: Vec<SpanData> = all_spans
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

            let open = matching
                .iter()
                .filter(|s| s.end_time <= s.start_time)
                .count();
            if open > 0 {
                return Err(TraceProviderError::SessionInProgress {
                    session_id: session_id.to_string(),
                    open_spans: open,
                });
            }

            Ok(RawSession::OtelSpans {
                session_id: session_id.to_string(),
                spans: matching,
            })
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

// ─── OTLP-JSON wire types ───────────────────────────────────────────────────

/// Top-level OTLP-HTTP+JSON export response body.
///
/// Only the fields the mappers downstream actually consume are parsed;
/// unknown fields are ignored by serde so backend-specific extensions
/// (e.g. `traceData`, pagination cursors) don't break deserialization.
#[derive(Debug, Default, Deserialize)]
struct OtlpTraceResponse {
    #[serde(default, rename = "resourceSpans")]
    resource_spans: Vec<OtlpResourceSpans>,
}

#[derive(Debug, Default, Deserialize)]
struct OtlpResourceSpans {
    #[serde(default)]
    resource: Option<OtlpResource>,
    #[serde(default, rename = "scopeSpans")]
    scope_spans: Vec<OtlpScopeSpans>,
}

#[derive(Debug, Default, Deserialize)]
struct OtlpResource {
    #[serde(default)]
    attributes: Vec<OtlpKeyValue>,
}

#[derive(Debug, Default, Deserialize)]
struct OtlpScopeSpans {
    #[serde(default)]
    scope: Option<OtlpInstrumentationScope>,
    #[serde(default)]
    spans: Vec<OtlpSpan>,
}

#[derive(Debug, Default, Deserialize)]
struct OtlpInstrumentationScope {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OtlpSpan {
    #[serde(default, rename = "traceId")]
    trace_id: String,
    #[serde(default, rename = "spanId")]
    span_id: String,
    #[serde(default, rename = "parentSpanId")]
    parent_span_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    kind: Option<i32>,
    #[serde(default, rename = "startTimeUnixNano")]
    start_time_unix_nano: OtlpUnixNano,
    #[serde(default, rename = "endTimeUnixNano")]
    end_time_unix_nano: OtlpUnixNano,
    #[serde(default)]
    attributes: Vec<OtlpKeyValue>,
}

/// Unix-nanos values in OTLP-JSON are conventionally encoded as strings
/// (to avoid loss of precision in languages without u64 JSON numbers),
/// though some producers emit numbers. Accept both.
#[derive(Debug, Default)]
struct OtlpUnixNano(u64);

impl<'de> Deserialize<'de> for OtlpUnixNano {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            S(String),
            N(u64),
        }
        match Raw::deserialize(d)? {
            Raw::N(n) => Ok(Self(n)),
            Raw::S(s) => s
                .parse::<u64>()
                .map(Self)
                .map_err(|e| D::Error::custom(format!("not u64: {e}"))),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct OtlpKeyValue {
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: OtlpAnyValue,
}

#[derive(Debug, Default, Deserialize)]
#[allow(clippy::struct_field_names)] // OTLP-JSON wire names end in `Value`.
struct OtlpAnyValue {
    #[serde(default, rename = "stringValue")]
    string_value: Option<String>,
    #[serde(default, rename = "intValue")]
    int_value: Option<OtlpIntValue>,
    #[serde(default, rename = "doubleValue")]
    double_value: Option<f64>,
    #[serde(default, rename = "boolValue")]
    bool_value: Option<bool>,
}

/// Ints on the OTLP-JSON wire are strings-or-numbers, same as unix-nanos.
#[derive(Debug, Default)]
struct OtlpIntValue(i64);

impl<'de> Deserialize<'de> for OtlpIntValue {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            S(String),
            N(i64),
        }
        match Raw::deserialize(d)? {
            Raw::N(n) => Ok(Self(n)),
            Raw::S(s) => s
                .parse::<i64>()
                .map(Self)
                .map_err(|e| D::Error::custom(format!("not i64: {e}"))),
        }
    }
}

impl OtlpTraceResponse {
    fn into_span_data(self) -> Vec<SpanData> {
        let mut out = Vec::new();
        for rs in self.resource_spans {
            // Resource attributes are promoted onto every emitted span so
            // mappers see them alongside span-level attributes.
            let resource_attrs: Vec<KeyValue> = rs
                .resource
                .map(|r| {
                    r.attributes
                        .into_iter()
                        .filter_map(kv_to_keyvalue)
                        .collect()
                })
                .unwrap_or_default();

            for ss in rs.scope_spans {
                let scope_name = ss
                    .scope
                    .as_ref()
                    .map(|s| s.name.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("otlp-http-pull")
                    .to_string();
                let scope_version = ss.scope.as_ref().and_then(|s| s.version.clone());
                for span in ss.spans {
                    out.push(otlp_span_to_span_data(
                        span,
                        &resource_attrs,
                        &scope_name,
                        scope_version.as_deref(),
                    ));
                }
            }
        }
        out
    }
}

fn otlp_span_to_span_data(
    span: OtlpSpan,
    resource_attrs: &[KeyValue],
    scope_name: &str,
    scope_version: Option<&str>,
) -> SpanData {
    let trace_id = hex_trace_id(&span.trace_id).unwrap_or(TraceId::INVALID);
    let span_id = hex_span_id(&span.span_id).unwrap_or(SpanId::INVALID);
    let parent_span_id = hex_span_id(&span.parent_span_id).unwrap_or(SpanId::INVALID);
    let span_context = SpanContext::new(
        trace_id,
        span_id,
        TraceFlags::default(),
        false,
        TraceState::default(),
    );

    let start_time = nanos_to_systime(span.start_time_unix_nano.0);
    let end_time = nanos_to_systime(span.end_time_unix_nano.0);

    let mut attributes: Vec<KeyValue> = resource_attrs.to_vec();
    attributes.extend(span.attributes.into_iter().filter_map(kv_to_keyvalue));

    let mut scope = InstrumentationScope::builder(scope_name.to_string());
    if let Some(v) = scope_version {
        scope = scope.with_version(v.to_string());
    }

    SpanData {
        span_context,
        parent_span_id,
        parent_span_is_remote: false,
        span_kind: otlp_kind_to_span_kind(span.kind),
        name: Cow::Owned(span.name),
        start_time,
        end_time,
        attributes,
        dropped_attributes_count: 0,
        events: SpanEvents::default(),
        links: SpanLinks::default(),
        status: Status::Unset,
        instrumentation_scope: scope.build(),
    }
}

fn otlp_kind_to_span_kind(kind: Option<i32>) -> SpanKind {
    // OTLP SpanKind enum: 0=UNSPECIFIED, 1=INTERNAL, 2=SERVER, 3=CLIENT,
    // 4=PRODUCER, 5=CONSUMER. Anything else → Internal.
    match kind.unwrap_or(0) {
        2 => SpanKind::Server,
        3 => SpanKind::Client,
        4 => SpanKind::Producer,
        5 => SpanKind::Consumer,
        _ => SpanKind::Internal,
    }
}

fn kv_to_keyvalue(kv: OtlpKeyValue) -> Option<KeyValue> {
    if kv.key.is_empty() {
        return None;
    }
    let value = if let Some(s) = kv.value.string_value {
        Value::String(s.into())
    } else if let Some(i) = kv.value.int_value {
        Value::I64(i.0)
    } else if let Some(d) = kv.value.double_value {
        Value::F64(d)
    } else if let Some(b) = kv.value.bool_value {
        Value::Bool(b)
    } else {
        return None;
    };
    Some(KeyValue::new(kv.key, value))
}

fn nanos_to_systime(ns: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_nanos(ns)
}

fn hex_trace_id(hex: &str) -> Option<TraceId> {
    if hex.is_empty() {
        return None;
    }
    let bytes = decode_hex::<16>(hex)?;
    Some(TraceId::from_bytes(bytes))
}

fn hex_span_id(hex: &str) -> Option<SpanId> {
    if hex.is_empty() {
        return None;
    }
    let bytes = decode_hex::<8>(hex)?;
    Some(SpanId::from_bytes(bytes))
}

fn decode_hex<const N: usize>(hex: &str) -> Option<[u8; N]> {
    // OTLP-JSON encodes ids as lowercase hex without a `0x` prefix,
    // `2*N` chars wide. Strings shorter than `2*N` are left-padded with
    // zeros; longer strings fail.
    if hex.len() > 2 * N {
        return None;
    }
    let mut out = [0u8; N];
    // Right-align: parse from the back so a short hex id fills the
    // low-order bytes (matches the OTel big-endian convention).
    let bytes = hex.as_bytes();
    let mut i = bytes.len();
    let mut j = N;
    while i >= 2 && j >= 1 {
        let byte = &bytes[i - 2..i];
        let hi = hex_digit(byte[0])?;
        let lo = hex_digit(byte[1])?;
        out[j - 1] = (hi << 4) | lo;
        i -= 2;
        j -= 1;
    }
    if i == 1 && j >= 1 {
        let lo = hex_digit(bytes[0])?;
        out[j - 1] = lo;
    }
    Some(out)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_hex_16_round_trips() {
        let bytes: [u8; 16] = decode_hex("0123456789abcdef0123456789abcdef").unwrap();
        assert_eq!(
            bytes,
            [
                0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab,
                0xcd, 0xef,
            ]
        );
    }

    #[test]
    fn decode_hex_rejects_oversize() {
        let r: Option<[u8; 8]> = decode_hex("0123456789abcdef00");
        assert!(r.is_none());
    }

    #[test]
    fn decode_hex_rejects_non_hex() {
        let r: Option<[u8; 8]> = decode_hex("zzzz567890abcdef");
        assert!(r.is_none());
    }

    #[test]
    fn nanos_to_systime_monotonic() {
        let a = nanos_to_systime(1_700_000_000_000_000_000);
        let b = nanos_to_systime(1_700_000_000_000_000_001);
        assert!(b > a);
    }
}
