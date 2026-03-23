//! HTTP status code classification for LLM provider error handling.
//!
//! Provides default and customizable mapping from HTTP status codes
//! to [`HttpErrorKind`] variants, which adapters can use to generate
//! appropriate error events.
//!
//! The [`error_event_from_status`] helper converts an HTTP error response
//! into an [`AssistantMessageEvent::Error`] with the correct
//! [`StreamErrorKind`], eliminating duplicated status-matching logic across
//! adapters.
//!
//! **Stability note:** This module is a shared implementation detail for
//! built-in adapters. External `StreamFn` implementors should depend only
//! on `swink_agent` (core) types. Breaking changes to this module's API
//! may occur without a major version bump.

use swink_agent::stream::AssistantMessageEvent;

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
                assert_eq!(
                    error_kind,
                    Some(swink_agent::stream::StreamErrorKind::Auth)
                );
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
                assert_eq!(
                    error_kind,
                    Some(swink_agent::stream::StreamErrorKind::Throttled)
                );
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
                assert_eq!(
                    error_kind,
                    Some(swink_agent::stream::StreamErrorKind::Network)
                );
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
                assert_eq!(
                    error_kind,
                    Some(swink_agent::stream::StreamErrorKind::Network)
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
}
