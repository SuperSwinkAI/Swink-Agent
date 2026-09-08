//! OAuth2 token refresh helpers.

use std::fmt;
use std::future::Future;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use swink_agent::CredentialError;
use tracing::debug;

/// Response from an OAuth2 token endpoint.
#[non_exhaustive]
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

/// `OAuth2` client configuration for a credential key with no stored
/// credential yet (US4: initial authorization flow).
///
/// Builds an authorization URL and exchanges the resulting code for tokens.
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
    /// Send a PKCE `S256` challenge (RFC 7636) with the authorization
    /// request and the matching verifier with the code exchange.
    ///
    /// Off by default so existing providers are byte-identical. Required
    /// by every public-client flow under OAuth 2.1.
    pub use_pkce: bool,
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
            use_pkce: false,
        }
    }

    /// Enable PKCE (`S256`) for the authorization-code flow.
    #[must_use]
    pub const fn with_pkce(mut self) -> Self {
        self.use_pkce = true;
        self
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

// ─── PKCE (RFC 7636) ────────────────────────────────────────────────────────

/// A single-use PKCE code verifier (RFC 7636 §4.1).
///
/// Generated per authorization attempt and consumed by the matching code
/// exchange; it is never stored in an [`AuthorizationConfig`] because a
/// config is long-lived and a verifier is not. The verifier is a secret:
/// `Debug` redacts it and it never reaches a log line or error variant.
pub struct PkceVerifier(String);

impl PkceVerifier {
    /// Generate a fresh verifier: 32 random octets, base64url-encoded
    /// without padding, giving 43 characters drawn from the RFC 7636
    /// unreserved set (`[A-Za-z0-9._~-]`).
    #[must_use]
    pub fn generate() -> Self {
        let bytes: [u8; 32] = rand::random();
        Self(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }

    /// The `S256` code challenge: `BASE64URL(SHA256(verifier))`.
    #[must_use]
    pub fn challenge(&self) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(self.0.as_bytes()))
    }

    /// The raw verifier, for the token request only.
    fn secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PkceVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PkceVerifier([REDACTED])")
    }
}

/// Build the authorization URL for the given config and CSRF `state` token.
///
/// Appends `response_type=code`, `client_id`, `redirect_uri`, `scope`
/// (space-joined, omitted if empty), and `state` as properly percent-encoded
/// query parameters. Never adds PKCE parameters; the resolver uses
/// `authorization_url` with a per-attempt [`PkceVerifier`] when
/// [`AuthorizationConfig::use_pkce`] is set.
pub fn build_authorization_url(
    config: &AuthorizationConfig,
    state: &str,
) -> Result<String, CredentialError> {
    authorization_url(config, state, None)
}

/// [`build_authorization_url`], plus `code_challenge` and
/// `code_challenge_method=S256` when a verifier is supplied.
pub(crate) fn authorization_url(
    config: &AuthorizationConfig,
    state: &str,
    pkce: Option<&PkceVerifier>,
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
        if let Some(verifier) = pkce {
            pairs.append_pair("code_challenge", &verifier.challenge());
            pairs.append_pair("code_challenge_method", "S256");
        }
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
    exchange_code_with_pkce(
        client,
        token_url,
        code,
        client_id,
        client_secret,
        redirect_uri,
        None,
    )
    .await
}

/// [`exchange_code`], plus `code_verifier` in the token request when the
/// authorization URL was built with the same [`PkceVerifier`].
pub(crate) async fn exchange_code_with_pkce(
    client: &reqwest::Client,
    token_url: &str,
    code: &str,
    client_id: &str,
    client_secret: Option<&str>,
    redirect_uri: &str,
    pkce: Option<&PkceVerifier>,
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
    if let Some(verifier) = pkce {
        form.push(("code_verifier", verifier.secret()));
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
#[non_exhaustive]
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
#[path = "oauth2_tests.rs"]
mod tests;
