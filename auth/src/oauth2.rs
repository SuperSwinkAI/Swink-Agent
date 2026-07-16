//! OAuth2 token refresh helpers.

use std::fmt;
use std::future::Future;
use std::time::{Duration, Instant};

use serde::Deserialize;
use swink_agent::CredentialError;
use tracing::debug;

/// Response from an OAuth2 token endpoint.
#[derive(Deserialize)]
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

impl fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenResponse")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_in", &self.expires_in)
            .field("token_type", &self.token_type)
            .finish()
    }
}

/// Standard OAuth2 error response body (RFC 6749 §5.2).
///
/// Only the stable `error` code is surfaced from a token endpoint failure.
/// `error_description` is intentionally ignored because providers can place
/// sensitive details there.
#[derive(Debug, Deserialize)]
struct OAuth2ErrorBody {
    error: String,
    #[serde(default)]
    _error_description: Option<String>,
}

/// Build a sanitized token-endpoint failure reason string from the HTTP
/// status and optional response body, for the given `action` (e.g. `"token
/// refresh"`, `"authorization code exchange"`).
///
/// The returned string NEVER includes the raw body verbatim. When the body
/// parses as an RFC 6749 §5.2 OAuth2 error response, only the stable `error`
/// code is included. All other bodies are ignored and only the status appears
/// in the surfaced reason.
///
/// The caller may emit redacted metadata such as body length via
/// `tracing::debug!`, but the raw body never reaches a surfaced
/// `CredentialError` reason and therefore never propagates into tool output.
fn sanitize_oauth2_error_reason(action: &str, status: reqwest::StatusCode, body: &str) -> String {
    // Attempt to parse a standard OAuth2 error response. Anything else
    // (HTML error pages, opaque vendor JSON, plain text, empty) degrades to a
    // status-only reason.
    if let Ok(parsed) = serde_json::from_str::<OAuth2ErrorBody>(body) {
        format!(
            "{action} failed: HTTP {} ({})",
            status.as_u16(),
            parsed.error
        )
    } else {
        format!("{action} failed: HTTP {}", status.as_u16())
    }
}

/// Sanitized reason for a failed `refresh_token` call. Thin wrapper over
/// [`sanitize_oauth2_error_reason`] preserving the historical `"token
/// refresh failed: ..."` wording.
fn sanitize_refresh_reason(status: reqwest::StatusCode, body: &str) -> String {
    sanitize_oauth2_error_reason("token refresh", status, body)
}

/// Sanitized reason for a failed `exchange_code` call.
fn sanitize_code_exchange_reason(status: reqwest::StatusCode, body: &str) -> String {
    sanitize_oauth2_error_reason("authorization code exchange", status, body)
}

/// Sanitized reason for a failed `request_device_code` call.
fn sanitize_device_authorization_reason(status: reqwest::StatusCode, body: &str) -> String {
    sanitize_oauth2_error_reason("device authorization request", status, body)
}

/// Sanitized reason for a terminal `poll_device_token` failure.
fn sanitize_device_token_reason(status: reqwest::StatusCode, body: &str) -> String {
    sanitize_oauth2_error_reason("device token request", status, body)
}

fn sanitize_token_endpoint(token_url: &str) -> String {
    match reqwest::Url::parse(token_url) {
        Ok(url) => {
            let mut endpoint = format!("{}://", url.scheme());
            endpoint.push_str(url.host_str().unwrap_or("<unknown-host>"));
            if let Some(port) = url.port() {
                endpoint.push(':');
                endpoint.push_str(&port.to_string());
            }
            if url.path() == "/" {
                endpoint.push('/');
            } else {
                endpoint.push_str("/<path>");
            }
            endpoint
        }
        Err(_) => "invalid-url".to_string(),
    }
}

fn sanitize_transport_reason(action: &str, error: &reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect failure"
    } else if error.is_request() {
        "request failure"
    } else if error.is_body() {
        "body failure"
    } else if error.is_decode() {
        "decode failure"
    } else {
        "failure"
    };
    format!("{action} failed: transport {kind}")
}

/// Perform an OAuth2 token refresh via the token endpoint.
///
/// Sends a POST request with `grant_type=refresh_token` to the given
/// `token_url`. Returns the parsed token response on success.
///
/// On failure, the returned [`CredentialError::RefreshFailed`] contains only
/// a sanitized reason: HTTP status plus (if the body is a standard OAuth2
/// error JSON) the stable `error` code. The raw response body is NEVER
/// included in the surfaced error, and debug logs only emit redacted metadata
/// so body contents cannot leak into user-visible tool output.
pub async fn refresh_token(
    client: &reqwest::Client,
    token_url: &str,
    refresh_token: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<TokenResponse, CredentialError> {
    debug!(
        token_endpoint = %sanitize_token_endpoint(token_url),
        "refreshing OAuth2 token"
    );

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
            reason: sanitize_transport_reason("token refresh", &e),
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        // Raw body stays in debug tracing only; never surfaced in the error
        // reason to avoid leaking token-endpoint payloads into tool output.
        // Include the sanitized endpoint here in addition to the initial
        // "refreshing" event: the initial event can be dropped when the
        // default subscriber isn't attached to reqwest's worker thread,
        // but this one always fires on the test's thread after `.await`.
        debug!(
            token_endpoint = %sanitize_token_endpoint(token_url),
            status = %status,
            body_len = body.len(),
            "OAuth2 token refresh failed; response body redacted"
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
            reason: sanitize_transport_reason("token refresh", &e),
        })
}

/// `OAuth2` client configuration needed to construct an authorization URL and
/// exchange the resulting code for tokens, for a credential key that has no
/// stored credential yet (US4: initial authorization flow).
///
/// This is distinct from [`Credential::OAuth2`](swink_agent::Credential::OAuth2)
/// (which describes an *already-issued* token set): a credential key must be
/// paired with an `AuthorizationConfig` via
/// [`with_authorization_config`](crate::DefaultCredentialResolver::with_authorization_config)
/// before the resolver can build an authorization URL for it. A key with an
/// authorization handler configured but no matching `AuthorizationConfig`
/// behaves as if no handler were configured (FR-011: `NotFound`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AuthorizationConfig {
    /// The provider's authorization endpoint (where the user is sent to
    /// grant access), e.g. `https://accounts.google.com/o/oauth2/v2/auth`.
    pub authorization_endpoint: String,
    /// The token endpoint used to exchange the authorization code for
    /// tokens.
    pub token_url: String,
    /// `OAuth2` client identifier.
    pub client_id: String,
    /// `OAuth2` client secret (optional for public clients).
    pub client_secret: Option<String>,
    /// The redirect URI registered with the provider; the authorization
    /// handler is responsible for listening on this address.
    pub redirect_uri: String,
    /// Requested scopes.
    pub scopes: Vec<String>,
}

impl AuthorizationConfig {
    /// Create a config from the required fields: authorization endpoint,
    /// token endpoint, client identifier, and redirect URI.
    ///
    /// The client secret defaults to `None` (public client) and the scope
    /// list to empty; set them with
    /// [`with_client_secret`](Self::with_client_secret) and
    /// [`with_scopes`](Self::with_scopes).
    #[must_use]
    pub fn new(
        authorization_endpoint: impl Into<String>,
        token_url: impl Into<String>,
        client_id: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        Self {
            authorization_endpoint: authorization_endpoint.into(),
            token_url: token_url.into(),
            client_id: client_id.into(),
            client_secret: None,
            redirect_uri: redirect_uri.into(),
            scopes: Vec::new(),
        }
    }

    /// Set the `OAuth2` client secret (confidential clients).
    #[must_use]
    pub fn with_client_secret(mut self, client_secret: impl Into<String>) -> Self {
        self.client_secret = Some(client_secret.into());
        self
    }

    /// Replace the requested scopes.
    #[must_use]
    pub fn with_scopes(mut self, scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.scopes = scopes.into_iter().map(Into::into).collect();
        self
    }
}

/// Build the authorization URL for the given config and CSRF `state` token.
///
/// Appends `response_type=code`, `client_id`, `redirect_uri`, `scope`
/// (space-joined, omitted if empty), and `state` as properly percent-encoded
/// query parameters.
pub fn build_authorization_url(
    config: &AuthorizationConfig,
    state: &str,
) -> Result<String, CredentialError> {
    let mut url = reqwest::Url::parse(&config.authorization_endpoint).map_err(|_| {
        CredentialError::AuthorizationFailed {
            key: String::new(),
            reason: "invalid authorization endpoint URL".to_string(),
        }
    })?;
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("response_type", "code");
        pairs.append_pair("client_id", &config.client_id);
        pairs.append_pair("redirect_uri", &config.redirect_uri);
        if !config.scopes.is_empty() {
            pairs.append_pair("scope", &config.scopes.join(" "));
        }
        pairs.append_pair("state", state);
    }
    Ok(url.to_string())
}

/// Exchange an `OAuth2` authorization code for tokens.
///
/// Sends a POST request with `grant_type=authorization_code` to `token_url`.
/// Returns the parsed token response on success.
///
/// On failure, the returned [`CredentialError::AuthorizationFailed`] contains
/// only a sanitized reason (mirroring [`refresh_token`]'s hygiene): HTTP
/// status plus (if the body is a standard OAuth2 error JSON) the stable
/// `error` code. The raw response body is NEVER included in the surfaced
/// error.
pub async fn exchange_code(
    client: &reqwest::Client,
    token_url: &str,
    code: &str,
    client_id: &str,
    client_secret: Option<&str>,
    redirect_uri: &str,
) -> Result<TokenResponse, CredentialError> {
    debug!(
        token_endpoint = %sanitize_token_endpoint(token_url),
        "exchanging OAuth2 authorization code for tokens"
    );

    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret));
    }

    let response = client
        .post(token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| CredentialError::AuthorizationFailed {
            key: String::new(),
            reason: sanitize_transport_reason("authorization code exchange", &e),
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        // Raw body stays in debug tracing only; see refresh_token's comment
        // on why this second debug! call (after the .await) is needed.
        debug!(
            token_endpoint = %sanitize_token_endpoint(token_url),
            status = %status,
            body_len = body.len(),
            "OAuth2 authorization code exchange failed; response body redacted"
        );
        let reason = sanitize_code_exchange_reason(status, &body);
        return Err(CredentialError::AuthorizationFailed {
            key: String::new(),
            reason,
        });
    }

    response
        .json::<TokenResponse>()
        .await
        .map_err(|e| CredentialError::AuthorizationFailed {
            key: String::new(),
            reason: sanitize_transport_reason("authorization code exchange", &e),
        })
}

// ─── Device authorization grant (RFC 8628) ──────────────────────────────────

/// Poll interval used when the provider omits `interval` (RFC 8628 §3.2
/// makes it OPTIONAL and §3.5 specifies 5 seconds as the default).
const DEFAULT_DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Amount the poll interval grows on each `slow_down` error (RFC 8628 §3.5:
/// "increase ... by 5 seconds").
const SLOW_DOWN_INTERVAL_INCREMENT: Duration = Duration::from_secs(5);

/// Fallback device-code lifetime when the provider omits `expires_in`.
/// RFC 8628 §3.2 marks `expires_in` REQUIRED, so this only guards against
/// non-conforming providers rather than defining normal behavior.
const DEFAULT_DEVICE_CODE_LIFETIME: Duration = Duration::from_secs(600);

/// The `grant_type` that identifies a device access token request
/// (RFC 8628 §3.4).
const DEVICE_CODE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// `OAuth2` client configuration for the device authorization grant
/// (RFC 8628), the headless counterpart to [`AuthorizationConfig`].
///
/// Unlike [`AuthorizationConfig`] this has no `redirect_uri` (the device flow
/// has no redirect) and its first-leg endpoint is the provider's *device
/// authorization endpoint*, which is distinct from the authorization endpoint
/// a browser is sent to.
///
/// Pair a credential key with one of these via
/// [`with_device_authorization_config`](crate::DefaultCredentialResolver::with_device_authorization_config).
/// A key with a device-code handler configured but no matching
/// `DeviceAuthorizationConfig` behaves as if no handler were configured
/// (FR-011: `NotFound`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DeviceAuthorizationConfig {
    /// The provider's device authorization endpoint, e.g.
    /// `https://oauth2.googleapis.com/device/code`.
    pub device_authorization_endpoint: String,
    /// The token endpoint polled for the issued tokens.
    pub token_url: String,
    /// `OAuth2` client identifier.
    pub client_id: String,
    /// `OAuth2` client secret (optional; device-flow clients are usually
    /// public clients).
    pub client_secret: Option<String>,
    /// Requested scopes.
    pub scopes: Vec<String>,
}

impl DeviceAuthorizationConfig {
    /// Create a config from the required fields: device authorization
    /// endpoint, token endpoint, and client identifier.
    ///
    /// The client secret defaults to `None` (device-flow clients are usually
    /// public) and the scope list to empty; set them with
    /// [`with_client_secret`](Self::with_client_secret) and
    /// [`with_scopes`](Self::with_scopes).
    #[must_use]
    pub fn new(
        device_authorization_endpoint: impl Into<String>,
        token_url: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Self {
        Self {
            device_authorization_endpoint: device_authorization_endpoint.into(),
            token_url: token_url.into(),
            client_id: client_id.into(),
            client_secret: None,
            scopes: Vec::new(),
        }
    }

    /// Set the `OAuth2` client secret (confidential clients).
    #[must_use]
    pub fn with_client_secret(mut self, client_secret: impl Into<String>) -> Self {
        self.client_secret = Some(client_secret.into());
        self
    }

    /// Replace the requested scopes.
    #[must_use]
    pub fn with_scopes(mut self, scopes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.scopes = scopes.into_iter().map(Into::into).collect();
        self
    }
}

/// A successful device authorization response (RFC 8628 §3.2).
#[derive(Deserialize)]
pub struct DeviceAuthorizationResponse {
    /// The secret the client polls the token endpoint with. Never shown to
    /// the user and redacted from [`Debug`].
    pub device_code: String,
    /// The short code the user types at [`Self::verification_uri`].
    pub user_code: String,
    /// The URL the user visits to enter [`Self::user_code`].
    pub verification_uri: String,
    /// Optional URL embedding the user code (RFC 8628 §3.3.1).
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    /// Lifetime of the device/user code pair in seconds.
    #[serde(default)]
    pub expires_in: Option<i64>,
    /// Minimum seconds between polls. Absent (or non-positive) means the
    /// RFC 8628 §3.5 default of 5 seconds.
    #[serde(default)]
    pub interval: Option<i64>,
}

impl fmt::Debug for DeviceAuthorizationResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `device_code` is a bearer-equivalent secret. `user_code` is meant
        // to be displayed to the user, so it stays visible.
        f.debug_struct("DeviceAuthorizationResponse")
            .field("device_code", &"<redacted>")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("verification_uri_complete", &self.verification_uri_complete)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

impl DeviceAuthorizationResponse {
    /// The poll interval, falling back to [`DEFAULT_DEVICE_POLL_INTERVAL`]
    /// when the provider omits or reports a non-positive `interval`.
    fn poll_interval(&self) -> Duration {
        match self.interval {
            Some(secs) if secs > 0 => Duration::from_secs(
                u64::try_from(secs).unwrap_or(DEFAULT_DEVICE_POLL_INTERVAL.as_secs()),
            ),
            _ => DEFAULT_DEVICE_POLL_INTERVAL,
        }
    }

    /// The device-code lifetime, falling back to
    /// [`DEFAULT_DEVICE_CODE_LIFETIME`] when the provider omits or reports a
    /// non-positive `expires_in`.
    fn lifetime(&self) -> Duration {
        match self.expires_in {
            Some(secs) if secs > 0 => Duration::from_secs(
                u64::try_from(secs).unwrap_or(DEFAULT_DEVICE_CODE_LIFETIME.as_secs()),
            ),
            Some(_) => Duration::ZERO,
            None => DEFAULT_DEVICE_CODE_LIFETIME,
        }
    }
}

/// Classification of a non-success device token endpoint response
/// (RFC 8628 §3.5).
#[derive(Debug, PartialEq, Eq)]
enum DevicePollOutcome {
    /// `authorization_pending` — the user hasn't finished yet; poll again at
    /// the current interval.
    Pending,
    /// `slow_down` — poll again, but increase the interval first.
    SlowDown,
    /// A terminal failure carrying an already-sanitized reason.
    Failed(String),
}

/// Classify a non-success response from the device token endpoint.
///
/// RFC 8628 §3.5 overloads HTTP 400 to mean "keep polling"
/// (`authorization_pending`, `slow_down`) as well as "give up"
/// (`access_denied`, `expired_token`), so the decision is driven by the
/// OAuth2 `error` code rather than the status. Bodies that don't parse as an
/// RFC 6749 §5.2 error response are terminal, with a status-only reason.
///
/// Like every other reason in this module, the returned string never contains
/// the raw body — only the status and the stable `error` code.
fn classify_device_poll_response(status: reqwest::StatusCode, body: &str) -> DevicePollOutcome {
    match serde_json::from_str::<OAuth2ErrorBody>(body) {
        Ok(parsed) => match parsed.error.as_str() {
            "authorization_pending" => DevicePollOutcome::Pending,
            "slow_down" => DevicePollOutcome::SlowDown,
            _ => DevicePollOutcome::Failed(sanitize_device_token_reason(status, body)),
        },
        Err(_) => DevicePollOutcome::Failed(sanitize_device_token_reason(status, body)),
    }
}

/// Request a device code and user code from the provider's device
/// authorization endpoint (RFC 8628 §3.1).
///
/// This is the first leg of the device grant; pass the result to
/// [`poll_device_token`] to obtain tokens.
///
/// On failure, the returned [`CredentialError::AuthorizationFailed`] contains
/// only a sanitized reason (mirroring [`exchange_code`]'s hygiene): HTTP
/// status plus (if the body is a standard OAuth2 error JSON) the stable
/// `error` code. The raw response body is NEVER included in the surfaced
/// error.
pub async fn request_device_code(
    client: &reqwest::Client,
    config: &DeviceAuthorizationConfig,
) -> Result<DeviceAuthorizationResponse, CredentialError> {
    debug!(
        device_authorization_endpoint = %sanitize_token_endpoint(&config.device_authorization_endpoint),
        "requesting OAuth2 device code"
    );

    let scopes = config.scopes.join(" ");
    let mut form = vec![("client_id", config.client_id.as_str())];
    if !scopes.is_empty() {
        form.push(("scope", scopes.as_str()));
    }
    if let Some(secret) = config.client_secret.as_deref() {
        form.push(("client_secret", secret));
    }

    let response = client
        .post(&config.device_authorization_endpoint)
        .form(&form)
        .send()
        .await
        .map_err(|e| CredentialError::AuthorizationFailed {
            key: String::new(),
            reason: sanitize_transport_reason("device authorization request", &e),
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        // Raw body stays in debug tracing only; see refresh_token's comment
        // on why this second debug! call (after the .await) is needed.
        debug!(
            device_authorization_endpoint = %sanitize_token_endpoint(&config.device_authorization_endpoint),
            status = %status,
            body_len = body.len(),
            "OAuth2 device authorization request failed; response body redacted"
        );
        return Err(CredentialError::AuthorizationFailed {
            key: String::new(),
            reason: sanitize_device_authorization_reason(status, &body),
        });
    }

    response
        .json::<DeviceAuthorizationResponse>()
        .await
        .map_err(|e| CredentialError::AuthorizationFailed {
            key: String::new(),
            reason: sanitize_transport_reason("device authorization request", &e),
        })
}

/// Poll the token endpoint until the user completes the device authorization
/// (RFC 8628 §3.4, §3.5).
///
/// Honors the provider's `interval`, backs off by 5 seconds on each
/// `slow_down`, and keeps polling on `authorization_pending`. Returns once
/// tokens are issued, or with [`CredentialError::AuthorizationFailed`] when
/// the provider reports a terminal error (e.g. `access_denied`,
/// `expired_token`) or the device code's own `expires_in` elapses.
///
/// This bounds itself by the device code's lifetime only. Callers wanting a
/// shorter overall bound should wrap the call in `tokio::time::timeout` — the
/// resolver does exactly that with its authorization timeout (FR-020).
///
/// As with the other helpers here, surfaced reasons never include the raw
/// response body.
pub async fn poll_device_token(
    client: &reqwest::Client,
    config: &DeviceAuthorizationConfig,
    device: &DeviceAuthorizationResponse,
) -> Result<TokenResponse, CredentialError> {
    poll_device_token_with_sleep(client, config, device, tokio::time::sleep).await
}

/// [`poll_device_token`] with an injectable sleep, so the polling loop's
/// interval and back-off behavior can be tested without real time passing.
async fn poll_device_token_with_sleep<S, F>(
    client: &reqwest::Client,
    config: &DeviceAuthorizationConfig,
    device: &DeviceAuthorizationResponse,
    sleep: S,
) -> Result<TokenResponse, CredentialError>
where
    S: Fn(Duration) -> F,
    F: Future<Output = ()>,
{
    let mut interval = device.poll_interval();
    let lifetime = device.lifetime();
    let started = Instant::now();

    let mut form = vec![
        ("grant_type", DEVICE_CODE_GRANT_TYPE),
        ("device_code", device.device_code.as_str()),
        ("client_id", config.client_id.as_str()),
    ];
    if let Some(secret) = config.client_secret.as_deref() {
        form.push(("client_secret", secret));
    }

    debug!(
        token_endpoint = %sanitize_token_endpoint(&config.token_url),
        interval_secs = interval.as_secs(),
        "polling OAuth2 device token endpoint"
    );

    loop {
        // Wait before each poll, including the first: the user needs time to
        // visit the verification URI, and RFC 8628 §3.5 requires polling no
        // faster than `interval`.
        sleep(interval).await;

        if started.elapsed() >= lifetime {
            return Err(CredentialError::AuthorizationFailed {
                key: String::new(),
                reason: "device token request failed: device code expired".to_string(),
            });
        }

        let response = client
            .post(&config.token_url)
            .form(&form)
            .send()
            .await
            .map_err(|e| CredentialError::AuthorizationFailed {
                key: String::new(),
                reason: sanitize_transport_reason("device token request", &e),
            })?;

        if response.status().is_success() {
            return response.json::<TokenResponse>().await.map_err(|e| {
                CredentialError::AuthorizationFailed {
                    key: String::new(),
                    reason: sanitize_transport_reason("device token request", &e),
                }
            });
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        debug!(
            token_endpoint = %sanitize_token_endpoint(&config.token_url),
            status = %status,
            body_len = body.len(),
            "OAuth2 device token poll returned an error; response body redacted"
        );

        match classify_device_poll_response(status, &body) {
            DevicePollOutcome::Pending => {}
            DevicePollOutcome::SlowDown => {
                interval = interval.saturating_add(SLOW_DOWN_INTERVAL_INCREMENT);
                debug!(
                    interval_secs = interval.as_secs(),
                    "device token endpoint asked us to slow down; increasing poll interval"
                );
            }
            DevicePollOutcome::Failed(reason) => {
                return Err(CredentialError::AuthorizationFailed {
                    key: String::new(),
                    reason,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use super::*;
    use reqwest::StatusCode;
    use tracing_subscriber::fmt::MakeWriter;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const LEAK_SENTINEL: &str = "LEAK_SENTINEL_ABC123";

    /// Build a plain client after installing the ring crypto provider —
    /// under `rustls-no-provider` (#1110) a bare `reqwest::Client::new()`
    /// panics until a process default provider exists.
    fn test_client() -> reqwest::Client {
        crate::ensure_default_crypto_provider();
        reqwest::Client::new()
    }

    #[derive(Clone, Default)]
    struct SharedLogBuffer(Arc<Mutex<Vec<u8>>>);

    impl SharedLogBuffer {
        fn contents(&self) -> String {
            let bytes = self.0.lock().unwrap().clone();
            String::from_utf8(bytes).unwrap()
        }
    }

    struct SharedLogWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedLogWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for SharedLogBuffer {
        type Writer = SharedLogWriter;

        fn make_writer(&'a self) -> Self::Writer {
            SharedLogWriter(Arc::clone(&self.0))
        }
    }

    /// Pin tracing's global max level to `DEBUG` for the whole test binary.
    ///
    /// The log-capture tests below install only *scoped* (thread-local)
    /// subscribers. With no global default, tracing's global `MAX_LEVEL`
    /// fast-path — which `debug!` consults before dispatching — flickers as
    /// scoped guards are set and dropped across the test harness's worker
    /// threads under a shared-process runner (`cargo test`), so a `debug!`
    /// can be filtered out before reaching the capture buffer and the
    /// assertion fails intermittently.
    ///
    /// Installing a global default at `DEBUG` once keeps `MAX_LEVEL` pinned
    /// for the binary's lifetime; the scoped capture subscriber still takes
    /// precedence on the test's own thread. The global writer is a sink, so
    /// it produces no output. Mirrors `plugins/web`'s `pin_global_info_level`
    /// (see #1094).
    fn pin_global_debug_level() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            let global = tracing_subscriber::fmt()
                .with_max_level(tracing::Level::DEBUG)
                .with_writer(io::sink)
                .finish();
            // Ignore the error if a global default was already set elsewhere;
            // any global default at DEBUG is enough to pin the level.
            let _ = tracing::subscriber::set_global_default(global);
        });
    }

    /// Install a capturing subscriber as this thread's default, with the
    /// global level pinned so the capture is deterministic under `cargo test`.
    fn capture_debug_logs(logs: &SharedLogBuffer) -> tracing::subscriber::DefaultGuard {
        pin_global_debug_level();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .without_time()
            .with_writer(logs.clone())
            .finish();
        tracing::subscriber::set_default(subscriber)
    }

    #[test]
    fn token_response_debug_redacts_tokens() {
        let response = TokenResponse {
            access_token: LEAK_SENTINEL.to_string(),
            refresh_token: Some("REFRESH_LEAK_SENTINEL".to_string()),
            expires_in: Some(3600),
            token_type: Some("Bearer".to_string()),
        };

        let debug = format!("{response:?}");

        assert!(
            !debug.contains(LEAK_SENTINEL),
            "access token leaked: {debug}"
        );
        assert!(
            !debug.contains("REFRESH_LEAK_SENTINEL"),
            "refresh token leaked: {debug}"
        );
        assert!(
            debug.contains("Bearer") && debug.contains("3600"),
            "safe token metadata should remain visible: {debug}"
        );
    }

    #[test]
    fn sanitized_reason_with_standard_oauth2_error_json_ignores_description() {
        let body = format!(
            r#"{{"error":"invalid_grant","error_description":"refresh token expired {LEAK_SENTINEL}"}}"#
        );
        let reason = sanitize_refresh_reason(StatusCode::UNAUTHORIZED, &body);

        assert_eq!(reason, "token refresh failed: HTTP 401 (invalid_grant)");
        assert!(
            !reason.contains(LEAK_SENTINEL),
            "error_description leaked into reason: {reason}"
        );
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

    #[test]
    fn sanitize_token_endpoint_redacts_query_and_path_details() {
        let endpoint = sanitize_token_endpoint(
            "https://user:pass@auth.example.com/token/refresh?client_secret=LEAK_SENTINEL_ABC123",
        );

        assert_eq!(endpoint, "https://auth.example.com/<path>");
        assert!(!endpoint.contains("user"));
        assert!(!endpoint.contains("pass"));
        assert!(!endpoint.contains("refresh"));
        assert!(!endpoint.contains("LEAK_SENTINEL_ABC123"));
    }

    #[tokio::test]
    async fn refresh_token_debug_log_redacts_response_body() {
        let mock_server = MockServer::start().await;
        let body = format!(
            r#"{{"error":"invalid_grant","error_description":"refresh token expired {LEAK_SENTINEL}"}}"#
        );
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(401).set_body_string(body))
            .mount(&mock_server)
            .await;
        let token_url = format!(
            "{}/token?client_secret={LEAK_SENTINEL}&tenant=swink",
            mock_server.uri()
        );

        let logs = SharedLogBuffer::default();
        let _guard = capture_debug_logs(&logs);

        let err = refresh_token(
            &test_client(),
            &token_url,
            "refresh-token",
            "client-id",
            Some("client-secret"),
        )
        .await
        .unwrap_err();
        let log_output = logs.contents();

        assert!(
            !format!("{err}").contains(LEAK_SENTINEL),
            "refresh error leaked sentinel: {err}"
        );
        assert!(
            !log_output.contains(LEAK_SENTINEL),
            "debug log leaked response body: {log_output}"
        );
        assert!(
            !log_output.contains("/token?"),
            "debug log leaked raw token endpoint: {log_output}"
        );
        assert!(
            log_output.contains("127.0.0.1")
                && log_output.contains("<path>")
                && !log_output.contains("client_secret"),
            "debug log should include a sanitized endpoint classification: {log_output}"
        );
        assert!(
            log_output.contains("body_len"),
            "debug log should include body length metadata: {log_output}"
        );
        assert!(
            log_output.contains("response body redacted"),
            "debug log should state the body is redacted: {log_output}"
        );
    }

    #[tokio::test]
    async fn refresh_token_transport_failure_reason_is_sanitized() {
        crate::ensure_default_crypto_provider();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();

        let err = refresh_token(
            &client,
            "http://127.0.0.1:1/token?client_secret=LEAK_SENTINEL_ABC123",
            "refresh-token",
            "client-id",
            Some("client-secret"),
        )
        .await
        .unwrap_err();
        let display = format!("{err}");

        assert!(
            !display.contains("LEAK_SENTINEL_ABC123"),
            "transport error leaked endpoint query details: {display}"
        );
        assert!(
            !display.contains("/token"),
            "transport error leaked endpoint path details: {display}"
        );
        assert!(
            display.contains("transport"),
            "transport error should use a stable sanitized reason: {display}"
        );
    }

    // T060: authorization code exchange

    #[tokio::test]
    async fn exchange_code_success_parses_token_response() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "exchanged-access",
                "refresh_token": "exchanged-refresh",
                "expires_in": 3600,
                "token_type": "Bearer"
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let token_url = format!("{}/token", mock_server.uri());
        let response = exchange_code(
            &test_client(),
            &token_url,
            "auth-code",
            "client-id",
            Some("client-secret"),
            "https://localhost:8080/callback",
        )
        .await
        .unwrap();

        assert_eq!(response.access_token, "exchanged-access");
        assert_eq!(response.refresh_token.as_deref(), Some("exchanged-refresh"));
    }

    #[tokio::test]
    async fn exchange_code_failure_returns_authorization_failed_without_leaking_body() {
        let mock_server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_grant",
                "error_description": format!("code expired {LEAK_SENTINEL}"),
            })))
            .mount(&mock_server)
            .await;

        let token_url = format!("{}/token", mock_server.uri());
        let err = exchange_code(
            &test_client(),
            &token_url,
            "auth-code",
            "client-id",
            Some("client-secret"),
            "https://localhost:8080/callback",
        )
        .await
        .unwrap_err();

        match &err {
            CredentialError::AuthorizationFailed { reason, .. } => {
                assert!(reason.contains("400"));
                assert!(reason.contains("invalid_grant"));
            }
            other => panic!("expected AuthorizationFailed, got {other:?}"),
        }
        let display = format!("{err}");
        assert!(
            !display.contains(LEAK_SENTINEL),
            "error_description leaked into Display: {display}"
        );
    }

    #[tokio::test]
    async fn exchange_code_transport_failure_reason_is_sanitized() {
        crate::ensure_default_crypto_provider();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .unwrap();

        let err = exchange_code(
            &client,
            "http://127.0.0.1:1/token?client_secret=LEAK_SENTINEL_ABC123",
            "auth-code",
            "client-id",
            Some("client-secret"),
            "https://localhost:8080/callback",
        )
        .await
        .unwrap_err();
        let display = format!("{err}");

        assert!(matches!(err, CredentialError::AuthorizationFailed { .. }));
        assert!(!display.contains("LEAK_SENTINEL_ABC123"));
        assert!(display.contains("transport"));
    }

    // T054 URL-construction check: authorize() must receive a correctly
    // formed authorization URL.
    #[test]
    fn build_authorization_url_includes_expected_query_params() {
        let config = AuthorizationConfig::new(
            "https://auth.example.com/o/authorize",
            "https://auth.example.com/token",
            "client with spaces",
            "http://localhost:8080/callback",
        )
        .with_client_secret("shh")
        .with_scopes(["read", "write"]);

        let url = build_authorization_url(&config, "csrf-state-123").unwrap();
        let parsed = reqwest::Url::parse(&url).unwrap();
        let pairs: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();

        assert_eq!(pairs.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(
            pairs.get("client_id").map(String::as_str),
            Some("client with spaces")
        );
        assert_eq!(
            pairs.get("redirect_uri").map(String::as_str),
            Some("http://localhost:8080/callback")
        );
        assert_eq!(pairs.get("scope").map(String::as_str), Some("read write"));
        assert_eq!(
            pairs.get("state").map(String::as_str),
            Some("csrf-state-123")
        );
        assert!(
            !url.contains("shh"),
            "client_secret must never appear in the authorization URL: {url}"
        );
    }

    #[test]
    fn build_authorization_url_omits_scope_when_empty() {
        let config = AuthorizationConfig::new(
            "https://auth.example.com/o/authorize",
            "https://auth.example.com/token",
            "client-1",
            "http://localhost:8080/callback",
        );

        let url = build_authorization_url(&config, "state").unwrap();
        assert!(!url.contains("scope="));
    }

    #[test]
    fn build_authorization_url_rejects_invalid_endpoint() {
        let config = AuthorizationConfig::new(
            "not a url",
            "https://auth.example.com/token",
            "client-1",
            "http://localhost:8080/callback",
        );

        let err = build_authorization_url(&config, "state").unwrap_err();
        assert!(matches!(err, CredentialError::AuthorizationFailed { .. }));
    }

    // ─── Device authorization grant (RFC 8628) ──────────────────────────────

    /// Build a sleeper for [`poll_device_token_with_sleep`] that records the
    /// durations it is asked to sleep for and returns immediately, making the
    /// loop's interval and back-off behavior observable without real time
    /// passing. Returns the recording handle alongside the sleeper.
    fn recording_sleeper() -> (
        Arc<Mutex<Vec<Duration>>>,
        impl Fn(Duration) -> std::future::Ready<()>,
    ) {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let handle = Arc::clone(&recorded);
        let sleeper = move |duration: Duration| {
            handle.lock().unwrap().push(duration);
            std::future::ready(())
        };
        (recorded, sleeper)
    }

    /// The recorded sleeps, in seconds, in the order the loop performed them.
    fn recorded_secs(recorded: &Arc<Mutex<Vec<Duration>>>) -> Vec<u64> {
        recorded
            .lock()
            .unwrap()
            .iter()
            .map(Duration::as_secs)
            .collect()
    }

    fn device_config(base_url: &str) -> DeviceAuthorizationConfig {
        DeviceAuthorizationConfig::new(
            format!("{base_url}/device/code"),
            format!("{base_url}/token"),
            "client-id",
        )
        .with_client_secret("client-secret")
        .with_scopes(["read"])
    }

    fn device_response(interval: Option<i64>) -> DeviceAuthorizationResponse {
        DeviceAuthorizationResponse {
            device_code: "device-code-secret".to_string(),
            user_code: "WDJB-MJHT".to_string(),
            verification_uri: "https://auth.example.com/device".to_string(),
            verification_uri_complete: None,
            expires_in: Some(600),
            interval,
        }
    }

    fn oauth2_error_response(code: &str) -> ResponseTemplate {
        ResponseTemplate::new(400).set_body_json(serde_json::json!({ "error": code }))
    }

    fn token_success_response() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "device-access",
            "refresh_token": "device-refresh",
            "expires_in": 3600,
            "token_type": "Bearer"
        }))
    }

    #[test]
    fn device_authorization_response_debug_redacts_device_code() {
        let response = DeviceAuthorizationResponse {
            device_code: LEAK_SENTINEL.to_string(),
            user_code: "WDJB-MJHT".to_string(),
            verification_uri: "https://auth.example.com/device".to_string(),
            verification_uri_complete: None,
            expires_in: Some(600),
            interval: Some(5),
        };

        let debug = format!("{response:?}");

        assert!(
            !debug.contains(LEAK_SENTINEL),
            "device_code leaked: {debug}"
        );
        assert!(
            debug.contains("WDJB-MJHT") && debug.contains("600"),
            "user-facing prompt fields should remain visible: {debug}"
        );
    }

    #[test]
    fn device_poll_interval_falls_back_when_absent_or_invalid() {
        assert_eq!(
            device_response(Some(3)).poll_interval(),
            Duration::from_secs(3)
        );
        assert_eq!(
            device_response(None).poll_interval(),
            DEFAULT_DEVICE_POLL_INTERVAL
        );
        assert_eq!(
            device_response(Some(0)).poll_interval(),
            DEFAULT_DEVICE_POLL_INTERVAL,
            "a non-positive interval must not busy-poll the provider"
        );
        assert_eq!(
            device_response(Some(-1)).poll_interval(),
            DEFAULT_DEVICE_POLL_INTERVAL
        );
    }

    #[test]
    fn device_lifetime_falls_back_when_expires_in_absent() {
        let mut response = device_response(None);
        response.expires_in = None;
        assert_eq!(response.lifetime(), DEFAULT_DEVICE_CODE_LIFETIME);

        response.expires_in = Some(30);
        assert_eq!(response.lifetime(), Duration::from_secs(30));

        response.expires_in = Some(0);
        assert_eq!(
            response.lifetime(),
            Duration::ZERO,
            "an already-expired code must not be treated as long-lived"
        );
    }

    #[test]
    fn classify_device_poll_response_recognizes_continuable_errors() {
        assert_eq!(
            classify_device_poll_response(
                StatusCode::BAD_REQUEST,
                r#"{"error":"authorization_pending"}"#
            ),
            DevicePollOutcome::Pending
        );
        assert_eq!(
            classify_device_poll_response(StatusCode::BAD_REQUEST, r#"{"error":"slow_down"}"#),
            DevicePollOutcome::SlowDown
        );
    }

    #[test]
    fn classify_device_poll_response_treats_denied_and_expired_as_terminal() {
        let denied =
            classify_device_poll_response(StatusCode::BAD_REQUEST, r#"{"error":"access_denied"}"#);
        assert_eq!(
            denied,
            DevicePollOutcome::Failed(
                "device token request failed: HTTP 400 (access_denied)".to_string()
            )
        );

        let expired =
            classify_device_poll_response(StatusCode::BAD_REQUEST, r#"{"error":"expired_token"}"#);
        assert_eq!(
            expired,
            DevicePollOutcome::Failed(
                "device token request failed: HTTP 400 (expired_token)".to_string()
            )
        );
    }

    #[test]
    fn classify_device_poll_response_redacts_body_in_terminal_reason() {
        // A pending-looking description must not rescue a terminal error, and
        // the description itself must never reach the reason.
        let body = format!(
            r#"{{"error":"access_denied","error_description":"user refused {LEAK_SENTINEL}"}}"#
        );
        let outcome = classify_device_poll_response(StatusCode::BAD_REQUEST, &body);

        match outcome {
            DevicePollOutcome::Failed(reason) => {
                assert!(!reason.contains(LEAK_SENTINEL), "sentinel leaked: {reason}");
                assert!(reason.contains("access_denied"));
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn classify_device_poll_response_treats_malformed_body_as_terminal() {
        let body = format!("<html>{LEAK_SENTINEL}</html>");
        let outcome = classify_device_poll_response(StatusCode::INTERNAL_SERVER_ERROR, &body);

        assert_eq!(
            outcome,
            DevicePollOutcome::Failed("device token request failed: HTTP 500".to_string()),
            "an unparseable body must not be mistaken for a continuable error"
        );
    }

    #[tokio::test]
    async fn request_device_code_success_parses_response() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/device/code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "device_code": "dev-123",
                "user_code": "WDJB-MJHT",
                "verification_uri": "https://auth.example.com/device",
                "verification_uri_complete": "https://auth.example.com/device?user_code=WDJB-MJHT",
                "expires_in": 1800,
                "interval": 5
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = device_config(&mock_server.uri());
        let response = request_device_code(&test_client(), &config).await.unwrap();

        assert_eq!(response.device_code, "dev-123");
        assert_eq!(response.user_code, "WDJB-MJHT");
        assert_eq!(
            response.verification_uri_complete.as_deref(),
            Some("https://auth.example.com/device?user_code=WDJB-MJHT")
        );
        assert_eq!(response.poll_interval(), Duration::from_secs(5));
        assert_eq!(response.lifetime(), Duration::from_secs(1800));
    }

    #[tokio::test]
    async fn request_device_code_failure_does_not_leak_body() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/device/code"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_client",
                "error_description": format!("bad client {LEAK_SENTINEL}"),
            })))
            .mount(&mock_server)
            .await;

        let config = device_config(&mock_server.uri());
        let err = request_device_code(&test_client(), &config)
            .await
            .unwrap_err();

        match &err {
            CredentialError::AuthorizationFailed { reason, .. } => {
                assert_eq!(
                    reason,
                    "device authorization request failed: HTTP 400 (invalid_client)"
                );
            }
            other => panic!("expected AuthorizationFailed, got {other:?}"),
        }
        assert!(!format!("{err}").contains(LEAK_SENTINEL));
    }

    #[tokio::test]
    async fn request_device_code_transport_failure_reason_is_sanitized() {
        crate::ensure_default_crypto_provider();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        let mut config = device_config("http://127.0.0.1:1");
        config.device_authorization_endpoint =
            "http://127.0.0.1:1/device/code?client_secret=LEAK_SENTINEL_ABC123".to_string();

        let err = request_device_code(&client, &config).await.unwrap_err();
        let display = format!("{err}");

        assert!(matches!(err, CredentialError::AuthorizationFailed { .. }));
        assert!(!display.contains("LEAK_SENTINEL_ABC123"));
        assert!(display.contains("transport"));
    }

    #[tokio::test]
    async fn poll_device_token_retries_while_authorization_pending() {
        let mock_server = MockServer::start().await;
        // Two pending polls, then success. `expect(2)` / `expect(1)` assert
        // the loop polled exactly the expected number of times.
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(oauth2_error_response("authorization_pending"))
            .up_to_n_times(2)
            .expect(2)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(token_success_response())
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = device_config(&mock_server.uri());
        let device = device_response(Some(3));
        let (recorded, sleeper) = recording_sleeper();

        let token = poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
            .await
            .unwrap();

        assert_eq!(token.access_token, "device-access");
        assert_eq!(token.refresh_token.as_deref(), Some("device-refresh"));
        assert_eq!(
            recorded_secs(&recorded),
            vec![3, 3, 3],
            "authorization_pending must not change the poll interval"
        );
    }

    #[tokio::test]
    async fn poll_device_token_backs_off_on_slow_down() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(oauth2_error_response("slow_down"))
            .up_to_n_times(2)
            .expect(2)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(token_success_response())
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = device_config(&mock_server.uri());
        let device = device_response(Some(5));
        let (recorded, sleeper) = recording_sleeper();

        let token = poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
            .await
            .unwrap();

        assert_eq!(token.access_token, "device-access");
        // Each slow_down adds 5s (RFC 8628 §3.5) and the increase persists
        // for every subsequent poll.
        assert_eq!(recorded_secs(&recorded), vec![5, 10, 15]);
    }

    #[tokio::test]
    async fn poll_device_token_interleaves_pending_and_slow_down() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(oauth2_error_response("authorization_pending"))
            .up_to_n_times(1)
            .expect(1)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(oauth2_error_response("slow_down"))
            .up_to_n_times(1)
            .expect(1)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(token_success_response())
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = device_config(&mock_server.uri());
        let device = device_response(Some(2));
        let (recorded, sleeper) = recording_sleeper();

        poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
            .await
            .unwrap();

        // Poll 1 pending (interval unchanged), poll 2 slow_down (bump to 7),
        // poll 3 succeeds.
        assert_eq!(recorded_secs(&recorded), vec![2, 2, 7]);
    }

    #[tokio::test]
    async fn poll_device_token_stops_on_access_denied() {
        let mock_server = MockServer::start().await;
        // `expect(1)` asserts the loop gives up rather than retrying a
        // terminal error.
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(oauth2_error_response("access_denied"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = device_config(&mock_server.uri());
        let device = device_response(Some(1));
        let (_recorded, sleeper) = recording_sleeper();

        let err = poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
            .await
            .unwrap_err();

        match &err {
            CredentialError::AuthorizationFailed { reason, .. } => {
                assert!(
                    reason.contains("access_denied"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected AuthorizationFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_device_token_stops_when_provider_reports_expired_token() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(oauth2_error_response("expired_token"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = device_config(&mock_server.uri());
        let device = device_response(Some(1));
        let (_recorded, sleeper) = recording_sleeper();

        let err = poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
            .await
            .unwrap_err();

        match &err {
            CredentialError::AuthorizationFailed { reason, .. } => {
                assert!(
                    reason.contains("expired_token"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected AuthorizationFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_device_token_stops_when_device_code_lifetime_elapses() {
        let mock_server = MockServer::start().await;
        // Never mounted to succeed: an already-expired code must fail before
        // any poll is issued.
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(oauth2_error_response("authorization_pending"))
            .expect(0)
            .mount(&mock_server)
            .await;

        let config = device_config(&mock_server.uri());
        let mut device = device_response(Some(1));
        device.expires_in = Some(0);
        let (_recorded, sleeper) = recording_sleeper();

        let err = poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
            .await
            .unwrap_err();

        match &err {
            CredentialError::AuthorizationFailed { reason, .. } => {
                assert_eq!(reason, "device token request failed: device code expired");
            }
            other => panic!("expected AuthorizationFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn poll_device_token_debug_log_redacts_response_body() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "access_denied",
                "error_description": format!("refused {LEAK_SENTINEL}"),
            })))
            .mount(&mock_server)
            .await;

        let logs = SharedLogBuffer::default();
        let _guard = capture_debug_logs(&logs);

        let config = device_config(&mock_server.uri());
        let device = device_response(Some(1));
        let (_recorded, sleeper) = recording_sleeper();

        let err = poll_device_token_with_sleep(&test_client(), &config, &device, sleeper)
            .await
            .unwrap_err();
        let log_output = logs.contents();

        assert!(
            !format!("{err}").contains(LEAK_SENTINEL),
            "poll error leaked sentinel: {err}"
        );
        assert!(
            !log_output.contains(LEAK_SENTINEL),
            "debug log leaked response body: {log_output}"
        );
        assert!(
            log_output.contains("body_len") && log_output.contains("response body redacted"),
            "debug log should record redacted body metadata: {log_output}"
        );
    }

    #[tokio::test]
    async fn poll_device_token_transport_failure_reason_is_sanitized() {
        crate::ensure_default_crypto_provider();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(200))
            .build()
            .unwrap();
        let mut config = device_config("http://127.0.0.1:1");
        config.token_url =
            "http://127.0.0.1:1/token?client_secret=LEAK_SENTINEL_ABC123".to_string();
        let device = device_response(Some(1));
        let (_recorded, sleeper) = recording_sleeper();

        let err = poll_device_token_with_sleep(&client, &config, &device, sleeper)
            .await
            .unwrap_err();
        let display = format!("{err}");

        assert!(matches!(err, CredentialError::AuthorizationFailed { .. }));
        assert!(!display.contains("LEAK_SENTINEL_ABC123"));
        assert!(display.contains("transport"));
    }
}
