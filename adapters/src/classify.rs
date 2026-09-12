//! HTTP status code classification for LLM provider error handling.
//!
//! Provides default and customizable mapping from HTTP status codes
//! to `HttpErrorKind` variants, which adapters can use to generate
//! appropriate error events.
//!
//! The `error_event_from_status` helper converts an HTTP error response
//! into an error event with the correct `StreamErrorKind`, eliminating
//! duplicated status-matching logic across adapters.
//!
//! **Stability note:** This module is a shared implementation detail for
//! built-in adapters. External `StreamFn` implementors should depend only
//! on `swink_agent` (core) types. Breaking changes to this module's API
//! may occur without a major version bump.

use std::time::Duration;

use swink_agent::AssistantMessageEvent;

/// Classification of HTTP error status codes for LLM providers.
///
/// Maps to the error categories that the core agent loop understands:
/// authentication failures are terminal, throttling is retryable, and
/// network/server errors are retryable.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpErrorKind {
    /// Authentication or authorization failure (401, 403).
    Auth,
    /// Rate limit / throttle (429).
    Throttled,
    /// Server, timeout, or network-like error (408, 5xx).
    Network,
}

/// Default HTTP status to [`HttpErrorKind`] classification.
///
/// Provides sensible defaults that work for most providers. Individual
/// adapters can override specific status codes via
/// [`classify_with_overrides`].
#[must_use]
pub const fn classify_http_status(code: u16) -> Option<HttpErrorKind> {
    match code {
        401 | 403 => Some(HttpErrorKind::Auth),
        429 => Some(HttpErrorKind::Throttled),
        408 | 500..=599 => Some(HttpErrorKind::Network),
        _ => None,
    }
}

/// Classify with provider-specific overrides applied first.
///
/// Checks `overrides` before falling back to [`classify_http_status`].
#[must_use]
pub fn classify_with_overrides(
    code: u16,
    overrides: &[(u16, HttpErrorKind)],
) -> Option<HttpErrorKind> {
    for (override_code, kind) in overrides {
        if code == *override_code {
            return Some(kind.clone());
        }
    }
    classify_http_status(code)
}

/// Check whether a provider error message matches a documented
/// context-window-overflow wording.
///
/// Several providers report context overflow without a dedicated structured
/// error code — only a generic error type (Anthropic `invalid_request_error`,
/// Bedrock `validationException`, Google `INVALID_ARGUMENT`) plus a
/// documented message. Built-in adapters call this helper *at the adapter
/// edge*, scoped to the provider's error type, so the structured
/// `StreamErrorKind::ContextWindowExceeded` is attached to the error event
/// instead of leaving classification to the core loop's substring fallback
/// (which exists only for third-party `StreamFn` implementations).
#[must_use]
pub fn is_context_overflow_message(message: &str) -> bool {
    // Provider-documented wordings, lowercase.
    const PATTERNS: &[&str] = &[
        // Anthropic: "prompt is too long: 210510 tokens > 200000 maximum"
        "prompt is too long",
        // Anthropic / Bedrock: "input length and `max_tokens` exceed context limit"
        "exceed context limit",
        // Bedrock: "Input is too long for requested model."
        "input is too long",
        // Bedrock (Anthropic models): "too many input tokens"
        "too many input tokens",
        // OpenAI / Azure / Mistral: "This model's maximum context length is 128000 tokens..."
        "maximum context length",
        // OpenAI structured code, occasionally embedded in raw bodies
        "context_length_exceeded",
        "context length exceeded",
        // OpenAI (newer models): "Your input exceeds the context window of this model"
        "exceeds the context window",
        // Google: "The input token count (32000) exceeds the maximum number of tokens allowed (30720)."
        "exceeds the maximum number of tokens",
        // Mistral: "Prompt contains 40960 tokens, too large for model with 32768 maximum context length"
        "too large for model",
        // xAI: "This model's maximum prompt length is 131072 but the request contains 200000 tokens."
        "maximum prompt length",
    ];
    let lower = message.to_lowercase();
    PATTERNS.iter().any(|pattern| lower.contains(pattern))
}

/// Heuristically detect a provider "model retired/decommissioned" response.
///
/// Providers signal model retirement as HTTP 400/404/410 with a
/// provider-specific error body: OpenAI returns a `model_not_found` code,
/// Anthropic and Gemini return 404s naming the model, and retirement notices
/// use wording like "decommissioned", "deprecated", or "no longer supported".
/// Only status codes 400, 404, and 410 are considered; the body match is
/// case-insensitive.
///
/// Note this intentionally also matches never-existed model ids — providers
/// report retired and unknown models with the same error code, and both mean
/// "this provider will not serve this model".
#[must_use]
pub fn is_model_retired_response(status: u16, body: &str) -> bool {
    if !matches!(status, 400 | 404 | 410) {
        return false;
    }
    let body = body.to_ascii_lowercase();
    if body.contains("model_not_found") || body.contains("model_decommissioned") {
        return true;
    }
    body.contains("model")
        && (body.contains("decommission")
            || body.contains("deprecated")
            || body.contains("deprecation")
            || body.contains("retired")
            || body.contains("no longer supported")
            || body.contains("not found")
            || body.contains("does not exist"))
}

/// Detect provider quota headers that advertise a hard zero request allowance.
///
/// Some providers report "not entitled to this model" as HTTP 429 with a
/// rate-limit *limit* of zero. That is not transient throttling: retrying the
/// same model cannot succeed until the user changes model or plan.
#[must_use]
pub fn has_zero_rate_limit_allowance(headers: &reqwest::header::HeaderMap) -> bool {
    headers.iter().any(|(name, value)| {
        let name = name.as_str().to_ascii_lowercase();
        is_rate_limit_limit_header(&name)
            && value
                .to_str()
                .ok()
                .is_some_and(rate_limit_header_value_is_zero)
    })
}

fn is_rate_limit_limit_header(name: &str) -> bool {
    (name.contains("ratelimit") || name.contains("rate-limit"))
        && name.contains("limit")
        && !name.contains("remaining")
        && !name.contains("reset")
}

fn rate_limit_header_value_is_zero(value: &str) -> bool {
    value
        .trim()
        .parse::<f64>()
        .is_ok_and(|n| n.is_finite() && n == 0.0)
}

/// Convert an HTTP error response into an [`AssistantMessageEvent::Error`].
///
/// Uses the default [`classify_http_status`] mapping. The `provider` label
/// (e.g. `"OpenAI"`, `"Azure"`) is included in the error message for
/// diagnostics.
///
/// Returns a classified error event:
/// - 401/403 → `error_auth`
/// - 408     → `error_network`
/// - 429     → `error_throttled`
/// - 5xx     → `error_network`
/// - 400/404/410 matching [`is_model_retired_response`] → `error_model_retired`
/// - other   → generic `error` (unclassified)
#[must_use]
pub fn error_event_from_status(status: u16, body: &str, provider: &str) -> AssistantMessageEvent {
    error_event_from_status_with_overrides(status, body, provider, &[])
}

/// Like [`error_event_from_status`] but applies provider-specific overrides
/// before falling back to the default classification.
///
/// For example, Anthropic maps 529 (overloaded) to [`HttpErrorKind::Network`]:
///
/// ```ignore
/// error_event_from_status_with_overrides(
///     529, &body, "Anthropic",
///     &[(529, HttpErrorKind::Network)],
/// )
/// ```
#[must_use]
pub fn error_event_from_status_with_overrides(
    status: u16,
    body: &str,
    provider: &str,
    overrides: &[(u16, HttpErrorKind)],
) -> AssistantMessageEvent {
    let kind = classify_with_overrides(status, overrides);
    match kind {
        Some(HttpErrorKind::Auth) => AssistantMessageEvent::error_auth(format!(
            "{provider} auth error (HTTP {status}): {body}"
        )),
        Some(HttpErrorKind::Throttled) => AssistantMessageEvent::error_throttled(format!(
            "{provider} rate limit (HTTP {status}): {body}"
        )),
        Some(HttpErrorKind::Network) => AssistantMessageEvent::error_network(format!(
            "{provider} server error (HTTP {status}): {body}"
        )),
        None => {
            // Model retirement/decommission responses get a structured kind
            // so the loop can tell the user to switch models.
            if is_model_retired_response(status, body) {
                return AssistantMessageEvent::error_model_retired(format!(
                    "{provider} model retired or unavailable (HTTP {status}): {body}"
                ));
            }
            // 4xx client errors that aren't auth/throttle get a generic error
            // (no StreamErrorKind), other codes get network classification.
            if (400..500).contains(&status) {
                AssistantMessageEvent::error(format!(
                    "{provider} client error (HTTP {status}): {body}"
                ))
            } else {
                AssistantMessageEvent::error(format!("{provider} HTTP {status}: {body}"))
            }
        }
    }
}

/// Attach a retry-after hint to an already-classified error event.
///
/// Adapters typically read the `Retry-After` header (via
/// [`parse_retry_after`]) before consuming the response body, then classify
/// the status via [`error_event_from_status_with_overrides`] afterward. This
/// merges the two without needing an extra parameter on every classification
/// call. A no-op for any non-`Error` event.
#[must_use]
pub fn with_retry_after(
    event: AssistantMessageEvent,
    retry_after: Option<Duration>,
) -> AssistantMessageEvent {
    match event {
        AssistantMessageEvent::Error {
            stop_reason,
            error_message,
            usage,
            error_kind,
            ..
        } => AssistantMessageEvent::Error {
            stop_reason,
            error_message,
            usage,
            error_kind,
            retry_after,
        },
        other => other,
    }
}

/// Parse a `Retry-After` response header into a [`Duration`].
///
/// Supports both forms defined by RFC 9110 §10.2.3:
/// - **delay-seconds** — a non-negative integer number of seconds
///   (e.g. `"30"`).
/// - **HTTP-date** — an absolute timestamp (e.g.
///   `"Wed, 21 Oct 2026 07:28:00 GMT"`); the duration is computed as the
///   difference between that timestamp and now, clamped to zero if the
///   timestamp is already in the past.
///
/// Returns `None` if the header is absent, not valid UTF-8, or matches
/// neither form.
#[must_use]
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers.get(reqwest::header::RETRY_AFTER)?.to_str().ok()?;
    parse_retry_after_value(value)
}

/// Parse a raw `Retry-After` header value.
///
/// Split out from [`parse_retry_after`] so the parsing logic can be unit
/// tested without constructing a `HeaderMap`.
#[must_use]
pub fn parse_retry_after_value(value: &str) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let target = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let delta = target.with_timezone(&chrono::Utc) - chrono::Utc::now();
    Some(delta.to_std().unwrap_or(Duration::ZERO))
}

#[cfg(test)]
#[path = "classify_tests.rs"]
mod tests;
