//! OAuth2 token refresh helpers.

use serde::Deserialize;
use swink_agent::CredentialError;
use tracing::debug;

/// Response from an OAuth2 token endpoint.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    /// The new access token.
    pub access_token: String,
    /// Optional new refresh token (rotation).
    pub refresh_token: Option<String>,
    /// Token lifetime in seconds.
    pub expires_in: Option<i64>,
    /// Token type (usually "Bearer").
    #[serde(default)]
    pub token_type: Option<String>,
}

/// Standard OAuth2 error response body (RFC 6749 §5.2).
///
/// Used to extract a stable `error` code (and optional `error_description`)
/// from a token endpoint failure without surfacing the full raw body — which
/// may contain provider-specific diagnostic data that should not leak into
/// tool output or user-visible logs.
#[derive(Debug, Deserialize)]
struct OAuth2ErrorBody {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Build a sanitized refresh-failure reason string from the HTTP status and
/// optional response body.
///
/// The returned string NEVER includes the raw body verbatim. When the body
/// parses as an RFC 6749 §5.2 OAuth2 error response, the stable `error` code
/// (and a length-capped `error_description`, if present) is included. All
/// other bodies are ignored and only the status appears in the surfaced
/// reason.
///
/// The full body is expected to be emitted separately via `tracing::debug!`
/// by the caller so operators can still diagnose failures when debug logging
/// is enabled — but it never reaches `CredentialError::RefreshFailed.reason`
/// and therefore never propagates into tool output.
fn sanitize_refresh_reason(status: reqwest::StatusCode, body: &str) -> String {
    // Attempt to parse a standard OAuth2 error response. Anything else
    // (HTML error pages, opaque vendor JSON, plain text, empty) degrades to a
    // status-only reason.
    if let Ok(parsed) = serde_json::from_str::<OAuth2ErrorBody>(body) {
        // Cap the description to prevent a pathological vendor from stuffing
        // sensitive content into `error_description`.
        const DESCRIPTION_MAX: usize = 200;
        let trimmed_description = parsed.error_description.as_deref().map(|d| {
            if d.len() > DESCRIPTION_MAX {
                format!("{}…", &d[..DESCRIPTION_MAX])
            } else {
                d.to_string()
            }
        });

        if let Some(desc) = trimmed_description {
            format!(
                "token refresh failed: HTTP {} ({}: {})",
                status.as_u16(),
                parsed.error,
                desc
            )
        } else {
            format!(
                "token refresh failed: HTTP {} ({})",
                status.as_u16(),
                parsed.error
            )
        }
    } else {
        format!("token refresh failed: HTTP {}", status.as_u16())
    }
}

/// Perform an OAuth2 token refresh via the token endpoint.
///
/// Sends a POST request with `grant_type=refresh_token` to the given
/// `token_url`. Returns the parsed token response on success.
///
/// On failure, the returned [`CredentialError::RefreshFailed`] contains only
/// a sanitized reason: HTTP status plus (if the body is a standard OAuth2
/// error JSON) the stable `error` code and truncated description. The raw
/// response body is NEVER included in the surfaced error — it is emitted only
/// via `tracing::debug!` so it cannot leak into user-visible tool output.
pub async fn refresh_token(
    client: &reqwest::Client,
    token_url: &str,
    refresh_token: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<TokenResponse, CredentialError> {
    debug!(token_url, "refreshing OAuth2 token");

    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }

    let response = client
        .post(token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| CredentialError::RefreshFailed {
            key: String::new(),
            reason: format!("HTTP request failed: {e}"),
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        // Raw body stays in debug tracing only; never surfaced in the error
        // reason to avoid leaking token-endpoint payloads into tool output.
        debug!(
            status = %status,
            body_len = body.len(),
            body = %body,
            "OAuth2 token refresh failed; raw body held for debug only"
        );
        let reason = sanitize_refresh_reason(status, &body);
        return Err(CredentialError::RefreshFailed {
            key: String::new(),
            reason,
        });
    }

    response
        .json::<TokenResponse>()
        .await
        .map_err(|e| CredentialError::RefreshFailed {
            key: String::new(),
            reason: format!("failed to parse token response: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    const LEAK_SENTINEL: &str = "LEAK_SENTINEL_ABC123";

    #[test]
    fn sanitized_reason_with_standard_oauth2_error_json() {
        let body = format!(
            r#"{{"error":"invalid_grant","error_description":"refresh token expired {LEAK_SENTINEL}"}}"#
        );
        let reason = sanitize_refresh_reason(StatusCode::UNAUTHORIZED, &body);

        // Status and standard code are included.
        assert!(reason.contains("401"), "reason missing status: {reason}");
        assert!(
            reason.contains("invalid_grant"),
            "reason missing error code: {reason}"
        );
        // The description is preserved here because it is a stable field in
        // the RFC 6749 §5.2 contract; callers are responsible for not
        // stuffing secrets into `error_description`. The sentinel may appear
        // because it lives inside the standard description field, which is
        // intentionally surfaced — the leak test below uses a body OUTSIDE
        // the standard schema to prove non-standard fields never leak.
        assert!(reason.starts_with("token refresh failed:"));
    }

    #[test]
    fn sanitized_reason_with_oauth2_error_no_description() {
        let body = r#"{"error":"invalid_grant"}"#;
        let reason = sanitize_refresh_reason(StatusCode::BAD_REQUEST, body);
        assert_eq!(
            reason, "token refresh failed: HTTP 400 (invalid_grant)",
            "unexpected reason format"
        );
    }

    #[test]
    fn sanitized_reason_redacts_non_standard_json_body() {
        // Vendor JSON that is NOT an RFC 6749 §5.2 error response — contains
        // a sentinel that must never reach the surfaced reason.
        let body = format!(
            r#"{{"trace_id":"abc","internal_message":"db lookup failed {LEAK_SENTINEL}"}}"#
        );
        let reason = sanitize_refresh_reason(StatusCode::INTERNAL_SERVER_ERROR, &body);

        assert!(
            !reason.contains(LEAK_SENTINEL),
            "sentinel leaked into reason: {reason}"
        );
        assert_eq!(reason, "token refresh failed: HTTP 500");
    }

    #[test]
    fn sanitized_reason_redacts_malformed_body() {
        let body = format!("<html><body>internal error {LEAK_SENTINEL}</body></html>");
        let reason = sanitize_refresh_reason(StatusCode::BAD_GATEWAY, &body);

        assert!(
            !reason.contains(LEAK_SENTINEL),
            "sentinel leaked into reason: {reason}"
        );
        assert!(
            !reason.contains("html"),
            "body fragment leaked into reason: {reason}"
        );
        assert_eq!(reason, "token refresh failed: HTTP 502");
    }

    #[test]
    fn sanitized_reason_redacts_empty_body() {
        let reason = sanitize_refresh_reason(StatusCode::UNAUTHORIZED, "");
        assert_eq!(reason, "token refresh failed: HTTP 401");
    }

    #[test]
    fn sanitized_reason_caps_error_description_length() {
        let long_desc = "x".repeat(500);
        let body = format!(r#"{{"error":"invalid_grant","error_description":"{long_desc}"}}"#);
        let reason = sanitize_refresh_reason(StatusCode::UNAUTHORIZED, &body);

        // Cap is 200 chars + trailing ellipsis.
        assert!(
            reason.len() < 300,
            "reason should be length-capped, got {} chars: {reason}",
            reason.len()
        );
        assert!(reason.contains("…"), "reason missing truncation marker");
    }

    #[test]
    fn sanitized_reason_does_not_include_body_for_ignored_fields() {
        // A standard OAuth2 body with an extra sensitive field outside the
        // recognized schema. serde's default behavior ignores unknown fields,
        // so the sentinel in `debug_info` must NOT appear in the reason.
        let body =
            format!(r#"{{"error":"invalid_grant","debug_info":"raw token dump {LEAK_SENTINEL}"}}"#);
        let reason = sanitize_refresh_reason(StatusCode::UNAUTHORIZED, &body);

        assert!(
            !reason.contains(LEAK_SENTINEL),
            "non-standard field leaked into reason: {reason}"
        );
        assert!(reason.contains("invalid_grant"));
    }
}
