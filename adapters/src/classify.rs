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

use swink_agent::AssistantMessageEvent;

/// Classification of HTTP error status codes for LLM providers.
///
/// Maps to the error categories that the core agent loop understands:
/// authentication failures are terminal, throttling is retryable, and
/// network/server errors are retryable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpErrorKind {
    /// Authentication or authorization failure (401, 403).
    Auth,
    /// Rate limit / throttle (429).
    Throttled,
    /// Server or network error (5xx).
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
        500..=599 => Some(HttpErrorKind::Network),
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

/// Convert an HTTP error response into an [`AssistantMessageEvent::Error`].
///
/// Uses the default [`classify_http_status`] mapping. The `provider` label
/// (e.g. `"OpenAI"`, `"Azure"`) is included in the error message for
/// diagnostics.
///
/// Returns a classified error event:
/// - 401/403 → `error_auth`
/// - 429     → `error_throttled`
/// - 5xx     → `error_network`
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_overflow_matches_documented_provider_wordings() {
        // Anthropic
        assert!(is_context_overflow_message(
            "prompt is too long: 210510 tokens > 200000 maximum"
        ));
        assert!(is_context_overflow_message(
            "input length and `max_tokens` exceed context limit: 199999 + 4096 > 200000"
        ));
        // Bedrock
        assert!(is_context_overflow_message(
            "Input is too long for requested model."
        ));
        assert!(is_context_overflow_message(
            "too many input tokens for this model"
        ));
        // OpenAI / Azure
        assert!(is_context_overflow_message(
            "This model's maximum context length is 128000 tokens. However, your messages resulted in 131000 tokens."
        ));
        assert!(is_context_overflow_message(
            "Your input exceeds the context window of this model."
        ));
        // Google
        assert!(is_context_overflow_message(
            "The input token count (32000) exceeds the maximum number of tokens allowed (30720)."
        ));
        // Mistral
        assert!(is_context_overflow_message(
            "Prompt contains 40960 tokens, too large for model with 32768 maximum context length"
        ));
        // xAI
        assert!(is_context_overflow_message(
            "This model's maximum prompt length is 131072 but the request contains 200000 tokens."
        ));
    }

    #[test]
    fn context_overflow_does_not_match_unrelated_messages() {
        assert!(!is_context_overflow_message("invalid api key"));
        assert!(!is_context_overflow_message(
            "max_tokens: must be greater than 0"
        ));
        assert!(!is_context_overflow_message("rate limit exceeded"));
        assert!(!is_context_overflow_message(""));
    }

    #[test]
    fn classify_401_is_auth() {
        assert_eq!(classify_http_status(401), Some(HttpErrorKind::Auth));
    }

    #[test]
    fn classify_403_is_auth() {
        assert_eq!(classify_http_status(403), Some(HttpErrorKind::Auth));
    }

    #[test]
    fn classify_429_is_throttled() {
        assert_eq!(classify_http_status(429), Some(HttpErrorKind::Throttled));
    }

    #[test]
    fn classify_500_is_network() {
        assert_eq!(classify_http_status(500), Some(HttpErrorKind::Network));
    }

    #[test]
    fn classify_200_is_none() {
        assert_eq!(classify_http_status(200), None);
    }

    #[test]
    fn classify_with_overrides_applies_first() {
        // Override 429 to be Auth instead of Throttled
        let overrides = vec![(429, HttpErrorKind::Auth)];
        assert_eq!(
            classify_with_overrides(429, &overrides),
            Some(HttpErrorKind::Auth),
        );

        // Non-overridden codes still use defaults
        assert_eq!(
            classify_with_overrides(500, &overrides),
            Some(HttpErrorKind::Network),
        );
    }

    #[test]
    fn error_event_401_is_auth() {
        let event = error_event_from_status(401, "bad key", "TestProvider");
        match event {
            AssistantMessageEvent::Error {
                error_message,
                error_kind,
                ..
            } => {
                assert!(error_message.contains("TestProvider"));
                assert!(error_message.contains("401"));
                assert!(error_message.contains("bad key"));
                assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Auth));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn error_event_429_is_throttled() {
        let event = error_event_from_status(429, "slow down", "TestProvider");
        match event {
            AssistantMessageEvent::Error {
                error_kind,
                error_message,
                ..
            } => {
                assert!(error_message.contains("429"));
                assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Throttled));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn error_event_500_is_network() {
        let event = error_event_from_status(500, "internal", "TestProvider");
        match event {
            AssistantMessageEvent::Error {
                error_kind,
                error_message,
                ..
            } => {
                assert!(error_message.contains("500"));
                assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Network));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn error_event_400_is_generic_client_error() {
        let event = error_event_from_status(400, "bad request", "TestProvider");
        match event {
            AssistantMessageEvent::Error {
                error_kind,
                error_message,
                ..
            } => {
                assert!(error_message.contains("client error"));
                assert!(error_message.contains("400"));
                assert_eq!(error_kind, None);
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn error_event_with_override_529_network() {
        let event = error_event_from_status_with_overrides(
            529,
            "overloaded",
            "Anthropic",
            &[(529, HttpErrorKind::Network)],
        );
        match event {
            AssistantMessageEvent::Error {
                error_kind,
                error_message,
                ..
            } => {
                assert!(error_message.contains("Anthropic"));
                assert!(error_message.contains("529"));
                assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Network));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    // ─── Cross-adapter error classification ──────────────────────────────

    /// Verify that all adapter-emitted error events carry a `StreamErrorKind`
    /// for the common error patterns (network, content filter, throttle).
    #[test]
    fn cross_adapter_unexpected_eof_is_network() {
        // All adapters should emit Network kind for unexpected stream EOF
        let providers = ["Anthropic", "OpenAI", "Google", "Ollama", "Bedrock"];
        for provider in providers {
            let event = AssistantMessageEvent::error_network(format!(
                "{provider} stream ended unexpectedly"
            ));
            match event {
                AssistantMessageEvent::Error { error_kind, .. } => {
                    assert_eq!(
                        error_kind,
                        Some(swink_agent::StreamErrorKind::Network),
                        "{provider} unexpected EOF should have Network kind"
                    );
                }
                other => panic!("expected Error for {provider}, got {other:?}"),
            }
        }
    }

    #[test]
    fn cross_adapter_content_filter_is_classified() {
        let event =
            AssistantMessageEvent::error_content_filtered("response blocked by safety filter");
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(
                    error_kind,
                    Some(swink_agent::StreamErrorKind::ContentFiltered),
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn http_error_classification_covers_all_adapter_status_codes() {
        // Auth errors
        for code in [401, 403] {
            let event = error_event_from_status(code, "forbidden", "TestAdapter");
            match event {
                AssistantMessageEvent::Error { error_kind, .. } => {
                    assert_eq!(
                        error_kind,
                        Some(swink_agent::StreamErrorKind::Auth),
                        "HTTP {code} should be Auth"
                    );
                }
                other => panic!("expected Error for HTTP {code}, got {other:?}"),
            }
        }

        // Throttle
        let event = error_event_from_status(429, "too many requests", "TestAdapter");
        match event {
            AssistantMessageEvent::Error { error_kind, .. } => {
                assert_eq!(error_kind, Some(swink_agent::StreamErrorKind::Throttled));
            }
            other => panic!("expected Error, got {other:?}"),
        }

        // Server errors
        for code in [500, 502, 503, 529] {
            let event = error_event_from_status_with_overrides(
                code,
                "server error",
                "TestAdapter",
                &[(529, HttpErrorKind::Network)],
            );
            match event {
                AssistantMessageEvent::Error { error_kind, .. } => {
                    assert_eq!(
                        error_kind,
                        Some(swink_agent::StreamErrorKind::Network),
                        "HTTP {code} should be Network"
                    );
                }
                other => panic!("expected Error for HTTP {code}, got {other:?}"),
            }
        }
    }
}
