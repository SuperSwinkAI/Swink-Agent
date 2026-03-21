//! HTTP status code classification for LLM provider error handling.
//!
//! Provides default and customizable mapping from HTTP status codes
//! to [`HttpErrorKind`] variants, which adapters can use to generate
//! appropriate error events.
//!
//! **Stability note:** This module is a shared implementation detail for
//! built-in adapters. External `StreamFn` implementors should depend only
//! on `swink_agent` (core) types. Breaking changes to this module's API
//! may occur without a major version bump.

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
}
