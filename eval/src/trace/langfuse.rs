//! `LangfuseTraceProvider` — pulls a trace (session) from the Langfuse
//! REST API and translates it into a [`RawSession`] (spec 043 T127).
//!
//! Langfuse stores *traces* (sessions) with a tree of *observations* —
//! each observation is typed (`SPAN`, `GENERATION`, `EVENT`) and carries
//! a `metadata` bag keyed by convention. Rather than invent a
//! Langfuse-specific [`crate::trace::mapper::SessionMapper`] variant, we
//! emit each observation as a generic OTel [`SpanData`] so any of the
//! three existing mappers (OpenInference / LangChain / OTel GenAI) can
//! consume the result — consumers decide which vocabulary their
//! instrumentation emits by populating `metadata` accordingly.
//!
//! Transport: `GET {base_url}/api/public/traces/{id}` with HTTP Basic
//! auth (public key : secret key), per the Langfuse public-API spec.
//! A 404 surfaces as [`TraceProviderError::SessionNotFound`]; any
//! observation with `endTime == null` (still in-flight) surfaces as
//! [`TraceProviderError::SessionInProgress`]. Other non-2xx responses
//! map to [`TraceProviderError::BackendFailure`].

#![cfg(feature = "trace-langfuse")]

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

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const LANGFUSE_SESSION_ATTRIBUTE: &str = "session.id";

/// Trace provider backed by the Langfuse public REST API.
///
/// Configure once with the deployment's `base_url` (e.g.
/// `https://cloud.langfuse.com`) plus the `public_key` / `secret_key`
/// pair. `fetch_session` treats `session_id` as a Langfuse *trace id*
/// and issues `GET {base}/api/public/traces/{id}`.
#[derive(Clone, Debug)]
pub struct LangfuseTraceProvider {
    inner: Arc<Inner>,
}

struct Inner {
    http: Client,
    base_url: String,
    public_key: String,
    secret_key: String,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("base_url", &self.base_url)
            .field("public_key", &"[REDACTED]")
            .field("secret_key", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl LangfuseTraceProvider {
    /// Build a provider targeting `base_url` authenticated with the
    /// supplied public/secret key pair.
    ///
    /// Trailing slashes on `base_url` are normalized away.
    ///
    /// # Errors
    /// Surfaces any `reqwest::Client::builder()` construction failure
    /// (e.g. TLS backend init) as [`TraceProviderError::BackendFailure`].
    pub fn new(
        base_url: impl Into<String>,
        public_key: impl Into<String>,
        secret_key: impl Into<String>,
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
                public_key: public_key.into(),
                secret_key: secret_key.into(),
            }),
        })
    }

    /// Base URL the provider targets (debug/introspection helper).
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }
}

// Manual Clone so `Arc::make_mut` would work if we ever add builder
// setters that mutate `Inner`. Keeps symmetry with
// [`crate::trace::otlp::OtlpHttpTraceProvider`].
impl Clone for Inner {
    fn clone(&self) -> Self {
        Self {
            http: self.http.clone(),
            base_url: self.base_url.clone(),
            public_key: self.public_key.clone(),
            secret_key: self.secret_key.clone(),
        }
    }
}

impl TraceProvider for LangfuseTraceProvider {
    fn fetch_session<'a>(
        &'a self,
        session_id: &'a str,
    ) -> crate::trace::provider::TraceProviderFuture<'a> {
        Box::pin(async move {
            let url = format!("{}/api/public/traces/{}", self.inner.base_url, session_id);
            let resp = self
                .inner
                .http
                .get(&url)
                .basic_auth(&self.inner.public_key, Some(&self.inner.secret_key))
                .send()
                .await
                .map_err(|err| TraceProviderError::BackendFailure {
                    reason: format!("langfuse GET {url}: {err}"),
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
                    reason: format!("langfuse http {}: {}", status.as_u16(), truncate(&body)),
                });
            }

            let trace: LangfuseTrace =
                resp.json()
                    .await
                    .map_err(|err| TraceProviderError::BackendFailure {
                        reason: format!("langfuse body parse: {err}"),
                    })?;

            let observations = trace.observations.unwrap_or_default();
            if observations.is_empty() {
                return Err(TraceProviderError::SessionNotFound {
                    session_id: session_id.to_string(),
                });
            }

            let open: usize = observations.iter().filter(|o| o.end_time.is_none()).count();
            if open > 0 {
                return Err(TraceProviderError::SessionInProgress {
                    session_id: session_id.to_string(),
                    open_spans: open,
                });
            }

            let spans = observations
                .into_iter()
                .map(|o| observation_to_span_data(o, session_id, trace.name.as_deref()))
                .collect();

            Ok(RawSession::OtelSpans {
                session_id: session_id.to_string(),
                spans,
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

// ─── Langfuse wire types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct LangfuseTrace {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    observations: Option<Vec<LangfuseObservation>>,
}

impl LangfuseTrace {
    #[allow(dead_code)]
    fn id(&self) -> &str {
        self.id.as_deref().unwrap_or_default()
    }
}

#[derive(Debug, Deserialize)]
struct LangfuseObservation {
    #[serde(default)]
    id: Option<String>,
    #[serde(default, rename = "parentObservationId")]
    parent_observation_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    /// `SPAN` | `GENERATION` | `EVENT`. We only use this hint to choose
    /// `SpanKind` — unrecognized types map to `Internal`.
    #[serde(default, rename = "type")]
    obs_type: Option<String>,
    #[serde(default, rename = "startTime")]
    start_time: Option<String>,
    #[serde(default, rename = "endTime")]
    end_time: Option<String>,
    /// Free-form user-supplied metadata. Each key is promoted to an OTel
    /// attribute on the emitted span so downstream mappers can consume it.
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    /// Langfuse-native fields a mapper may consult directly; surfaced as
    /// OTel attrs under a `langfuse.*` prefix so mappers that don't care
    /// can ignore them.
    #[serde(default)]
    model: Option<String>,
    #[serde(default, rename = "promptTokens")]
    prompt_tokens: Option<u64>,
    #[serde(default, rename = "completionTokens")]
    completion_tokens: Option<u64>,
    #[serde(default, rename = "totalTokens")]
    total_tokens: Option<u64>,
    #[serde(default)]
    output: Option<serde_json::Value>,
}

fn observation_to_span_data(
    obs: LangfuseObservation,
    session_id: &str,
    trace_name: Option<&str>,
) -> SpanData {
    let name = obs
        .name
        .or_else(|| trace_name.map(str::to_string))
        .unwrap_or_else(|| "langfuse.observation".to_string());

    let start = obs
        .start_time
        .as_deref()
        .and_then(parse_rfc3339)
        .unwrap_or_else(SystemTime::now);
    let end = obs
        .end_time
        .as_deref()
        .and_then(parse_rfc3339)
        .unwrap_or(start + Duration::from_millis(1));

    let span_id = obs.id.as_deref().map_or(SpanId::INVALID, hash_to_span_id);
    let parent_span_id = obs
        .parent_observation_id
        .as_deref()
        .map_or(SpanId::INVALID, hash_to_span_id);
    let trace_id = hash_to_trace_id(session_id);

    let span_context = SpanContext::new(
        trace_id,
        span_id,
        TraceFlags::default(),
        false,
        TraceState::default(),
    );

    let span_kind = match obs.obs_type.as_deref() {
        Some("GENERATION") => SpanKind::Client,
        _ => SpanKind::Internal,
    };

    let mut attributes: Vec<KeyValue> = Vec::new();
    attributes.push(KeyValue::new(
        LANGFUSE_SESSION_ATTRIBUTE,
        session_id.to_string(),
    ));
    if let Some(model) = obs.model {
        attributes.push(KeyValue::new("langfuse.model", model));
    }
    if let Some(t) = obs.prompt_tokens {
        attributes.push(KeyValue::new(
            "langfuse.usage.prompt_tokens",
            i64::try_from(t).unwrap_or(i64::MAX),
        ));
    }
    if let Some(t) = obs.completion_tokens {
        attributes.push(KeyValue::new(
            "langfuse.usage.completion_tokens",
            i64::try_from(t).unwrap_or(i64::MAX),
        ));
    }
    if let Some(t) = obs.total_tokens {
        attributes.push(KeyValue::new(
            "langfuse.usage.total_tokens",
            i64::try_from(t).unwrap_or(i64::MAX),
        ));
    }
    if let Some(out) = &obs.output
        && let Some(text) = output_text(out)
    {
        attributes.push(KeyValue::new("langfuse.output.text", text));
    }
    if let Some(meta) = obs.metadata
        && let Some(map) = meta.as_object()
    {
        for (k, v) in map {
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
        instrumentation_scope: InstrumentationScope::builder("langfuse").build(),
    }
}

/// Minimal RFC 3339 parser that covers the shapes Langfuse emits —
/// `YYYY-MM-DDTHH:MM:SS[.fraction][Z | +HH:MM | -HH:MM]`. Returns `None`
/// on anything we can't confidently parse; callers fall back to "now".
///
/// Hand-rolled to avoid pulling `chrono` into the eval default build
/// (SC-009). We only need seconds-precision round-trips for tests.
fn parse_rfc3339(s: &str) -> Option<SystemTime> {
    // Split into date portion, time portion, and timezone suffix.
    let (date, rest) = s.split_once('T')?;
    let mut date_parts = date.splitn(3, '-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;

    let (time_part, tz_part) = split_tz(rest)?;

    // Strip fractional seconds if present.
    let (hms, frac_nanos) = match time_part.split_once('.') {
        Some((hms, frac)) => (hms, parse_fraction_to_nanos(frac)),
        None => (time_part, 0),
    };
    let mut hms_parts = hms.splitn(3, ':');
    let hour: u32 = hms_parts.next()?.parse().ok()?;
    let minute: u32 = hms_parts.next()?.parse().ok()?;
    let second: u32 = hms_parts.next()?.parse().ok()?;

    let tz_offset_seconds = parse_tz_offset(tz_part)?;

    // Compute days since 1970-01-01 for (year, month, day) using Howard
    // Hinnant's `days_from_civil` — exact and branch-cheap.
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
    // Find a `+` or `-` that starts a timezone offset (must be followed
    // by digits-and-colon; not part of a negative fractional second).
    let bytes = rest.as_bytes();
    for (i, &b) in bytes.iter().enumerate().rev() {
        if (b == b'+' || b == b'-') && i > 0 {
            return Some((&rest[..i], &rest[i..]));
        }
    }
    None
}

fn parse_fraction_to_nanos(frac: &str) -> u32 {
    // Accept at most 9 digits; shorter fractions are padded, longer are truncated.
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

/// Howard Hinnant's `days_from_civil`: returns days since 1970-01-01
/// for any proleptic Gregorian date. Exact for the full `i64` range;
/// no dependency on `chrono`.
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

fn output_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => Some(v.to_string()),
        _ => None,
    }
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

// Langfuse ids are opaque strings (commonly UUIDs); fold them into an
// OTel `SpanId` / `TraceId` deterministically so round-trips are stable.
// We use SHA-256 and truncate — not cryptographic here, just used to
// give the OTel types non-zero bytes that preserve distinctness across
// different langfuse ids.
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
        let a = hash_to_span_id("obs-1");
        let b = hash_to_span_id("obs-1");
        assert_eq!(a, b);
        let c = hash_to_span_id("obs-2");
        assert_ne!(a, c);
    }

    #[test]
    fn langfuse_provider_debug_redacts_auth_keys() {
        let provider =
            LangfuseTraceProvider::new("https://langfuse.example", "pk-secret", "sk-secret")
                .expect("provider builds");

        let debug = format!("{provider:?}");

        assert!(
            !debug.contains("pk-secret"),
            "Debug leaks Langfuse public key"
        );
        assert!(
            !debug.contains("sk-secret"),
            "Debug leaks Langfuse secret key"
        );
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("https://langfuse.example"));
    }

    #[test]
    fn parse_rfc3339_roundtrips() {
        let t = parse_rfc3339("2026-04-23T10:00:00Z").expect("rfc3339");
        let later = parse_rfc3339("2026-04-23T10:00:01Z").expect("rfc3339");
        assert!(later > t);
    }

    #[test]
    fn json_to_otel_value_maps_primitives() {
        assert!(matches!(
            json_to_otel_value(&serde_json::json!("hi")),
            Some(Value::String(_))
        ));
        assert!(matches!(
            json_to_otel_value(&serde_json::json!(true)),
            Some(Value::Bool(true))
        ));
        assert!(matches!(
            json_to_otel_value(&serde_json::json!(42)),
            Some(Value::I64(42))
        ));
        assert!(matches!(
            json_to_otel_value(&serde_json::json!(2.5_f64)),
            Some(Value::F64(_))
        ));
        assert!(json_to_otel_value(&serde_json::Value::Null).is_none());
    }
}
