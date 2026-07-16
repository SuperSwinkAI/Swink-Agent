//! Credential types, traits, and error types for tool authentication.
//!
//! Tools declare authentication requirements via [`AuthConfig`]; the framework
//! resolves credentials from a pluggable [`CredentialStore`] and delivers the
//! resolved secret to `execute()` as an [`Option<ResolvedCredential>`].

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

// ─── Credential ─────────────────────────────────────────────────────────────

/// A secret value with type information for tool authentication.
#[non_exhaustive]
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Credential {
    /// A single secret API key string.
    ApiKey {
        /// The API key value.
        key: String,
    },
    /// A bearer token with optional expiry.
    Bearer {
        /// The bearer token value.
        token: String,
        /// When the token expires (if known).
        #[serde(default)]
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// A full `OAuth2` token set with refresh capability.
    OAuth2 {
        /// The current access token.
        access_token: String,
        /// Optional refresh token for automatic renewal.
        refresh_token: Option<String>,
        /// When the access token expires (if known).
        expires_at: Option<chrono::DateTime<chrono::Utc>>,
        /// Token endpoint URL for refresh requests.
        token_url: String,
        /// `OAuth2` client identifier.
        client_id: String,
        /// `OAuth2` client secret (optional for public clients).
        client_secret: Option<String>,
        /// Requested scopes.
        #[serde(default)]
        scopes: Vec<String>,
    },
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey { .. } => f
                .debug_struct("Credential::ApiKey")
                .field("key", &"[REDACTED]")
                .finish(),
            Self::Bearer { expires_at, .. } => f
                .debug_struct("Credential::Bearer")
                .field("token", &"[REDACTED]")
                .field("expires_at", expires_at)
                .finish(),
            Self::OAuth2 {
                expires_at,
                client_id,
                scopes,
                ..
            } => f
                .debug_struct("Credential::OAuth2")
                .field("access_token", &"[REDACTED]")
                .field("refresh_token", &"[REDACTED]")
                .field("expires_at", expires_at)
                .field("token_url", &"[REDACTED]")
                .field("client_id", client_id)
                .field("client_secret", &"[REDACTED]")
                .field("scopes", scopes)
                .finish(),
        }
    }
}

impl Credential {
    /// Returns the [`CredentialType`] discriminant for this credential.
    #[must_use]
    pub const fn credential_type(&self) -> CredentialType {
        match self {
            Self::ApiKey { .. } => CredentialType::ApiKey,
            Self::Bearer { .. } => CredentialType::Bearer,
            Self::OAuth2 { .. } => CredentialType::OAuth2,
        }
    }
}

// ─── ResolvedCredential ─────────────────────────────────────────────────────

/// Minimal secret value delivered to a tool after credential resolution.
///
/// Does NOT contain refresh tokens, client secrets, or token endpoints.
/// Tools receive only the secret they need for the authenticated request.
#[non_exhaustive]
#[derive(Clone)]
pub enum ResolvedCredential {
    /// A resolved API key.
    ApiKey(String),
    /// A resolved bearer token.
    Bearer(String),
    /// A resolved (possibly refreshed) `OAuth2` access token.
    OAuth2AccessToken(String),
}

impl std::fmt::Debug for ResolvedCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey(_) => f
                .debug_tuple("ResolvedCredential::ApiKey")
                .field(&"[REDACTED]")
                .finish(),
            Self::Bearer(_) => f
                .debug_tuple("ResolvedCredential::Bearer")
                .field(&"[REDACTED]")
                .finish(),
            Self::OAuth2AccessToken(_) => f
                .debug_tuple("ResolvedCredential::OAuth2AccessToken")
                .field(&"[REDACTED]")
                .finish(),
        }
    }
}

// ─── AuthConfig ─────────────────────────────────────────────────────────────

/// Per-tool declaration of authentication requirements.
///
/// Returned by [`AgentTool::auth_config()`](crate::AgentTool::auth_config) to
/// declare that a tool needs credentials resolved before execution.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Key to look up in the credential store.
    pub credential_key: String,
    /// How to attach the credential to the outbound request.
    pub auth_scheme: AuthScheme,
    /// Expected credential type (for mismatch checking).
    pub credential_type: CredentialType,
}

impl AuthConfig {
    /// Create a new auth config from its required fields.
    #[must_use]
    pub fn new(
        credential_key: impl Into<String>,
        auth_scheme: AuthScheme,
        credential_type: CredentialType,
    ) -> Self {
        Self {
            credential_key: credential_key.into(),
            auth_scheme,
            credential_type,
        }
    }
}

// ─── AuthScheme ─────────────────────────────────────────────────────────────

/// How a resolved credential is attached to the outbound request.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum AuthScheme {
    /// `Authorization: Bearer {token}`
    BearerHeader,
    /// `{header_name}: {key}`
    ApiKeyHeader(String),
    /// `?{param_name}={key}`
    ApiKeyQuery(String),
}

// ─── CredentialType ─────────────────────────────────────────────────────────

/// Credential type discriminant for mismatch checking (FR-018).
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialType {
    /// Expects an API key credential.
    ApiKey,
    /// Expects a bearer token.
    Bearer,
    /// Expects an `OAuth2` token set.
    OAuth2,
}

// ─── CredentialError ────────────────────────────────────────────────────────

/// Errors from credential resolution.
///
/// All variants include the credential key for diagnostics but NEVER include
/// secret values (FR-016).
#[non_exhaustive]
pub enum CredentialError {
    /// Credential not found in the store.
    NotFound {
        /// The credential key that was looked up.
        key: String,
    },

    /// Credential has expired and cannot be refreshed.
    Expired {
        /// The credential key that expired.
        key: String,
    },

    /// `OAuth2` token refresh failed.
    RefreshFailed {
        /// The credential key whose refresh failed.
        key: String,
        /// Human-readable reason (no secrets).
        reason: String,
    },

    /// Credential type doesn't match what the tool expects.
    TypeMismatch {
        /// The credential key.
        key: String,
        /// The type the tool declared.
        expected: CredentialType,
        /// The type found in the store.
        actual: CredentialType,
    },

    /// Generic credential store error.
    StoreError(Box<dyn std::error::Error + Send + Sync>),

    /// Credential resolution timed out.
    Timeout {
        /// The credential key.
        key: String,
    },

    /// The interactive `OAuth2` authorization code flow failed (handler
    /// error, or the code-for-token exchange was rejected).
    AuthorizationFailed {
        /// The credential key that was being authorized.
        key: String,
        /// Human-readable reason (no secrets).
        reason: String,
    },

    /// The user did not complete the interactive authorization flow within
    /// the configured timeout (FR-020, default 5 minutes).
    AuthorizationTimeout {
        /// The credential key that was being authorized.
        key: String,
    },
}

impl fmt::Debug for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { key } => f
                .debug_struct("CredentialError::NotFound")
                .field("key", key)
                .finish(),
            Self::Expired { key } => f
                .debug_struct("CredentialError::Expired")
                .field("key", key)
                .finish(),
            Self::RefreshFailed { key, reason } => f
                .debug_struct("CredentialError::RefreshFailed")
                .field("key", key)
                .field("reason", reason)
                .finish(),
            Self::TypeMismatch {
                key,
                expected,
                actual,
            } => f
                .debug_struct("CredentialError::TypeMismatch")
                .field("key", key)
                .field("expected", expected)
                .field("actual", actual)
                .finish(),
            Self::StoreError(_) => f
                .debug_tuple("CredentialError::StoreError")
                .field(&"[REDACTED]")
                .finish(),
            Self::Timeout { key } => f
                .debug_struct("CredentialError::Timeout")
                .field("key", key)
                .finish(),
            Self::AuthorizationFailed { key, reason } => f
                .debug_struct("CredentialError::AuthorizationFailed")
                .field("key", key)
                .field("reason", reason)
                .finish(),
            Self::AuthorizationTimeout { key } => f
                .debug_struct("CredentialError::AuthorizationTimeout")
                .field("key", key)
                .finish(),
        }
    }
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { key } => write!(f, "credential not found: {key}"),
            Self::Expired { key } => write!(f, "credential expired: {key}"),
            Self::RefreshFailed { key, reason } => {
                write!(f, "credential refresh failed for {key}: {reason}")
            }
            Self::TypeMismatch {
                key,
                expected,
                actual,
            } => write!(
                f,
                "credential type mismatch for {key}: expected {expected:?}, got {actual:?}"
            ),
            // Backend store failures may contain arbitrary vendor text, so the
            // user-facing `Display` output stays generic.
            Self::StoreError(_) => f.write_str("credential store error"),
            Self::Timeout { key } => write!(f, "credential resolution timed out for {key}"),
            Self::AuthorizationFailed { key, reason } => {
                write!(f, "authorization failed for {key}: {reason}")
            }
            Self::AuthorizationTimeout { key } => {
                write!(f, "authorization timed out for {key}")
            }
        }
    }
}

impl std::error::Error for CredentialError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::StoreError(error) => Some(&**error),
            _ => None,
        }
    }
}

impl Clone for CredentialError {
    fn clone(&self) -> Self {
        match self {
            Self::NotFound { key } => Self::NotFound { key: key.clone() },
            Self::Expired { key } => Self::Expired { key: key.clone() },
            Self::RefreshFailed { key, reason } => Self::RefreshFailed {
                key: key.clone(),
                reason: reason.clone(),
            },
            Self::TypeMismatch {
                key,
                expected,
                actual,
            } => Self::TypeMismatch {
                key: key.clone(),
                expected: *expected,
                actual: *actual,
            },
            Self::StoreError(error) => {
                Self::StoreError(Box::new(std::io::Error::other(error.to_string())))
            }
            Self::Timeout { key } => Self::Timeout { key: key.clone() },
            Self::AuthorizationFailed { key, reason } => Self::AuthorizationFailed {
                key: key.clone(),
                reason: reason.clone(),
            },
            Self::AuthorizationTimeout { key } => Self::AuthorizationTimeout { key: key.clone() },
        }
    }
}

/// Boxed async result used by credential traits.
pub type CredentialFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, CredentialError>> + Send + 'a>>;

// ─── CredentialStore trait ──────────────────────────────────────────────────

/// Pluggable credential storage abstraction.
///
/// Thread-safe for concurrent tool executions. Implementations must be
/// `Send + Sync` to allow sharing across `tokio::spawn` boundaries.
pub trait CredentialStore: Send + Sync {
    /// Retrieve a credential by key.
    fn get(&self, key: &str) -> CredentialFuture<'_, Option<Credential>>;

    /// Store or update a credential by key.
    fn set(&self, key: &str, credential: Credential) -> CredentialFuture<'_, ()>;

    /// Delete a credential by key.
    fn delete(&self, key: &str) -> CredentialFuture<'_, ()>;
}

// ─── CredentialResolver trait ───────────────────────────────────────────────

/// Orchestrator for credential resolution — checks validity, triggers
/// refresh, deduplicates concurrent requests.
pub trait CredentialResolver: Send + Sync {
    /// Resolve a credential by key. Returns the minimal secret value
    /// needed for the authenticated request.
    fn resolve(&self, key: &str) -> CredentialFuture<'_, ResolvedCredential>;
}

// ─── AuthorizationHandler trait ─────────────────────────────────────────────

/// Pluggable callback for initiating interactive `OAuth2` authorization code
/// flows (FR-010).
///
/// Implementations typically open a browser to `auth_url` and listen for the
/// provider's redirect on a local callback server, returning the resulting
/// authorization code. `state` is the CSRF token the resolver generated for
/// this attempt; implementations that run their own callback listener should
/// verify the redirect's `state` query parameter matches before trusting the
/// `code`.
///
/// When no handler is configured, a missing credential resolves to
/// [`CredentialError::NotFound`] instead of attempting interactive
/// authorization (FR-011).
pub trait AuthorizationHandler: Send + Sync {
    /// Present the authorization URL to the user and return the resulting
    /// authorization code.
    fn authorize(&self, auth_url: &str, state: &str) -> CredentialFuture<'_, String>;
}

// ─── DeviceCodeHandler trait ────────────────────────────────────────────────

/// User-facing instructions for an in-progress `OAuth2` device authorization
/// grant (RFC 8628 §3.2).
///
/// This carries only the fields a user needs to see. The `device_code` — the
/// secret the client polls the token endpoint with — is deliberately NOT
/// included: handlers display a prompt, they do not participate in polling,
/// so they never need it.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct DeviceCodePrompt {
    /// The short code the user types at the verification URI, e.g. `WDJB-MJHT`.
    pub user_code: String,
    /// The URL the user visits to enter `user_code`.
    pub verification_uri: String,
    /// Optional URL that embeds `user_code`, letting the user skip typing it
    /// (RFC 8628 §3.3.1). Handlers should prefer this when present.
    pub verification_uri_complete: Option<String>,
    /// Lifetime of `user_code` in seconds, if the provider reported one.
    pub expires_in: Option<i64>,
}

impl DeviceCodePrompt {
    /// Create a new device code prompt from its required fields.
    #[must_use]
    pub fn new(user_code: impl Into<String>, verification_uri: impl Into<String>) -> Self {
        Self {
            user_code: user_code.into(),
            verification_uri: verification_uri.into(),
            verification_uri_complete: None,
            expires_in: None,
        }
    }

    /// Set the URL that embeds `user_code` (RFC 8628 §3.3.1).
    #[must_use]
    pub fn with_verification_uri_complete(
        mut self,
        verification_uri_complete: impl Into<String>,
    ) -> Self {
        self.verification_uri_complete = Some(verification_uri_complete.into());
        self
    }

    /// Set the lifetime of `user_code` in seconds.
    #[must_use]
    pub const fn with_expires_in(mut self, expires_in: i64) -> Self {
        self.expires_in = Some(expires_in);
        self
    }
}

/// Pluggable callback for the `OAuth2` device authorization grant
/// (RFC 8628), the headless counterpart to [`AuthorizationHandler`].
///
/// Where [`AuthorizationHandler`] sends a user to a URL and must return an
/// authorization code, this handler only *displays* a prompt — the resolver
/// polls the token endpoint itself. That makes it suitable for CLI/TUI and
/// other contexts where an authorization-code redirect isn't practical, since
/// no local callback listener is required.
///
/// Implementations typically print `prompt.user_code` and
/// `prompt.verification_uri` (or open `verification_uri_complete` in a
/// browser) and return immediately. Returning from `present` does not signal
/// that the user has finished authorizing — the resolver keeps polling until
/// the provider issues a token, denies the request, or the code expires.
///
/// When no handler is configured, a missing credential resolves to
/// [`CredentialError::NotFound`] instead of attempting device authorization,
/// mirroring [`AuthorizationHandler`]'s behavior (FR-011).
pub trait DeviceCodeHandler: Send + Sync {
    /// Display the device authorization prompt to the user.
    ///
    /// Returning `Ok(())` means the prompt was shown, not that the user has
    /// authorized. Return an error to abort the flow before polling starts.
    fn present(&self, prompt: &DeviceCodePrompt) -> CredentialFuture<'_, ()>;
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    // T023: Credential serde roundtrip
    #[test]
    fn credential_serde_roundtrip_api_key() {
        let cred = Credential::ApiKey {
            key: "sk-test-123".into(),
        };
        let json = serde_json::to_string(&cred).unwrap();
        let decoded: Credential = serde_json::from_str(&json).unwrap();
        match decoded {
            Credential::ApiKey { key } => assert_eq!(key, "sk-test-123"),
            other => panic!("expected ApiKey, got {other:?}"),
        }
    }

    #[test]
    fn credential_serde_roundtrip_bearer() {
        let cred = Credential::Bearer {
            token: "tok-abc".into(),
            expires_at: Some(chrono::Utc::now()),
        };
        let json = serde_json::to_string(&cred).unwrap();
        let decoded: Credential = serde_json::from_str(&json).unwrap();
        match decoded {
            Credential::Bearer { token, expires_at } => {
                assert_eq!(token, "tok-abc");
                assert!(expires_at.is_some());
            }
            other => panic!("expected Bearer, got {other:?}"),
        }
    }

    #[test]
    fn credential_serde_roundtrip_oauth2() {
        let cred = Credential::OAuth2 {
            access_token: "access-123".into(),
            refresh_token: Some("refresh-456".into()),
            expires_at: None,
            token_url: "https://auth.example.com/token".into(),
            client_id: "client-1".into(),
            client_secret: Some("secret".into()),
            scopes: vec!["read".into(), "write".into()],
        };
        let json = serde_json::to_string(&cred).unwrap();
        let decoded: Credential = serde_json::from_str(&json).unwrap();
        match decoded {
            Credential::OAuth2 {
                access_token,
                refresh_token,
                client_id,
                scopes,
                ..
            } => {
                assert_eq!(access_token, "access-123");
                assert_eq!(refresh_token.as_deref(), Some("refresh-456"));
                assert_eq!(client_id, "client-1");
                assert_eq!(scopes, vec!["read", "write"]);
            }
            other => panic!("expected OAuth2, got {other:?}"),
        }
    }

    // T024: CredentialError Display contains no secrets
    #[test]
    fn credential_error_display_no_secrets() {
        let errors = vec![
            CredentialError::NotFound {
                key: "my-key".into(),
            },
            CredentialError::Expired {
                key: "my-key".into(),
            },
            CredentialError::RefreshFailed {
                key: "my-key".into(),
                reason: "bad response".into(),
            },
            CredentialError::TypeMismatch {
                key: "my-key".into(),
                expected: CredentialType::Bearer,
                actual: CredentialType::ApiKey,
            },
            CredentialError::Timeout {
                key: "my-key".into(),
            },
            CredentialError::AuthorizationFailed {
                key: "my-key".into(),
                reason: "user denied access".into(),
            },
            CredentialError::AuthorizationTimeout {
                key: "my-key".into(),
            },
        ];

        let secret_values = [
            "sk-test-123",
            "tok-abc",
            "access-123",
            "refresh-456",
            "secret",
        ];
        for err in &errors {
            let display = format!("{err}");
            for secret in &secret_values {
                assert!(
                    !display.contains(secret),
                    "Display of {err:?} leaks secret {secret}"
                );
            }
            // Should contain the key name for diagnostics
            assert!(
                display.contains("my-key"),
                "Display of {err:?} should contain key name"
            );
        }
    }

    #[test]
    fn credential_store_error_display_redacts_backend_details() {
        let err = CredentialError::StoreError(Box::new(std::io::Error::other(
            "backend exploded with token=secret-value",
        )));

        assert_eq!(err.to_string(), "credential store error");

        let source = err.source().expect("store errors should retain the source");
        assert!(
            source.to_string().contains("token=secret-value"),
            "store error source should keep the backend detail for internal diagnostics"
        );
    }

    #[test]
    fn credential_store_error_debug_redacts_backend_details() {
        let err = CredentialError::StoreError(Box::new(std::io::Error::other(
            "backend exploded with token=secret-value",
        )));

        let debug = format!("{err:?}");

        assert!(
            !debug.contains("token=secret-value"),
            "Debug leaks backend secret"
        );
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn oauth2_debug_redacts_token_url() {
        let cred = Credential::OAuth2 {
            access_token: "access-secret".into(),
            refresh_token: Some("refresh-secret".into()),
            expires_at: None,
            token_url: "https://client:token-secret@auth.example.com/token?api_key=query-secret"
                .into(),
            client_id: "client-1".into(),
            client_secret: Some("client-secret".into()),
            scopes: vec!["read".into()],
        };

        let debug = format!("{cred:?}");

        for secret in [
            "access-secret",
            "refresh-secret",
            "client-secret",
            "token-secret",
            "query-secret",
        ] {
            assert!(
                !debug.contains(secret),
                "Debug leaks OAuth2 secret {secret}"
            );
        }
        assert!(debug.contains("token_url"));
        assert!(debug.contains("[REDACTED]"));
    }

    // T011: credential_type helper
    #[test]
    fn credential_type_helper() {
        let api_key = Credential::ApiKey { key: "k".into() };
        assert_eq!(api_key.credential_type(), CredentialType::ApiKey);

        let bearer = Credential::Bearer {
            token: "t".into(),
            expires_at: None,
        };
        assert_eq!(bearer.credential_type(), CredentialType::Bearer);

        let oauth2 = Credential::OAuth2 {
            access_token: "a".into(),
            refresh_token: None,
            expires_at: None,
            token_url: "https://example.com/token".into(),
            client_id: "c".into(),
            client_secret: None,
            scopes: vec![],
        };
        assert_eq!(oauth2.credential_type(), CredentialType::OAuth2);
    }

    // T023 additional: Debug impl redacts secrets
    #[test]
    fn debug_impl_redacts_secrets() {
        let cred = Credential::ApiKey {
            key: "super-secret".into(),
        };
        let debug = format!("{cred:?}");
        assert!(!debug.contains("super-secret"), "Debug leaks secret");
        assert!(debug.contains("[REDACTED]"));

        let resolved = ResolvedCredential::ApiKey("my-secret".into());
        let debug = format!("{resolved:?}");
        assert!(!debug.contains("my-secret"), "Debug leaks secret");
        assert!(debug.contains("[REDACTED]"));
    }

    // T057/T058: new US4 error variants carry no secrets and surface the key
    // for diagnostics, matching the existing CredentialError hygiene pattern.
    #[test]
    fn authorization_failed_display_and_debug_contain_no_secrets() {
        let err = CredentialError::AuthorizationFailed {
            key: "google-calendar".into(),
            reason: "token endpoint rejected code: HTTP 400 (invalid_grant)".into(),
        };
        let display = format!("{err}");
        let debug = format!("{err:?}");
        assert!(display.contains("google-calendar"));
        assert!(debug.contains("google-calendar"));
        assert!(!display.contains("access_token"));
        assert!(!debug.contains("access_token"));
    }

    #[test]
    fn authorization_timeout_display_and_debug_contain_key() {
        let err = CredentialError::AuthorizationTimeout {
            key: "google-calendar".into(),
        };
        assert!(format!("{err}").contains("google-calendar"));
        assert!(format!("{err:?}").contains("google-calendar"));
    }

    #[test]
    // The clone is the behavior under test, not an accident.
    #[allow(clippy::redundant_clone)]
    fn authorization_error_clone_preserves_fields() {
        let failed = CredentialError::AuthorizationFailed {
            key: "k".into(),
            reason: "denied".into(),
        };
        match failed.clone() {
            CredentialError::AuthorizationFailed { key, reason } => {
                assert_eq!(key, "k");
                assert_eq!(reason, "denied");
            }
            other => panic!("expected AuthorizationFailed, got {other:?}"),
        }

        let timed_out = CredentialError::AuthorizationTimeout { key: "k".into() };
        match timed_out.clone() {
            CredentialError::AuthorizationTimeout { key } => assert_eq!(key, "k"),
            other => panic!("expected AuthorizationTimeout, got {other:?}"),
        }
    }
}
