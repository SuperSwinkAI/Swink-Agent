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

/// Perform an OAuth2 token refresh via the token endpoint.
///
/// Sends a POST request with `grant_type=refresh_token` to the given
/// `token_url`. Returns the parsed token response on success.
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
        return Err(CredentialError::RefreshFailed {
            key: String::new(),
            reason: format!("token endpoint returned {status}: {body}"),
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
