//! Default credential resolver with expiry checking, OAuth2 refresh, and
//! concurrent request deduplication.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use tokio::sync::Mutex;
use tracing::{debug, info};

use swink_agent::credential::CredentialFuture;
use swink_agent::{
    AuthorizationHandler, Credential, CredentialError, CredentialResolver, CredentialStore,
    ResolvedCredential,
};

use crate::oauth2;

/// A shared future for an in-flight credential refresh/resolution.
type InFlightFuture = Shared<BoxFuture<'static, Result<ResolvedCredential, String>>>;

/// Default credential resolver that handles:
/// - API key passthrough
/// - Bearer token expiry validation
/// - OAuth2 token refresh with deduplication
/// - OAuth2 authorization code flow (when handler is configured)
pub struct DefaultCredentialResolver {
    store: Arc<dyn CredentialStore>,
    client: reqwest::Client,
    expiry_buffer: Duration,
    authorization_handler: Option<Arc<dyn AuthorizationHandler>>,
    authorization_timeout: Duration,
    /// In-flight refresh futures keyed by credential key.
    /// Concurrent resolves for the same key share a single refresh request.
    in_flight: Mutex<HashMap<String, InFlightFuture>>,
}

impl DefaultCredentialResolver {
    /// Create a resolver backed by the given credential store.
    #[must_use]
    pub fn new(store: Arc<dyn CredentialStore>) -> Self {
        Self {
            store,
            client: reqwest::Client::new(),
            expiry_buffer: Duration::from_secs(60),
            authorization_handler: None,
            authorization_timeout: Duration::from_secs(300),
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    /// Set a custom HTTP client for OAuth2 refresh requests.
    #[must_use]
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    /// Set the expiry buffer (default: 60 seconds).
    ///
    /// Tokens expiring within this duration are treated as expired.
    #[must_use]
    pub fn with_expiry_buffer(mut self, buffer: Duration) -> Self {
        self.expiry_buffer = buffer;
        self
    }

    /// Set an authorization handler for interactive OAuth2 flows.
    #[must_use]
    pub fn with_authorization_handler(mut self, handler: Arc<dyn AuthorizationHandler>) -> Self {
        self.authorization_handler = Some(handler);
        self
    }

    /// Set the authorization flow timeout (default: 5 minutes).
    #[must_use]
    pub fn with_authorization_timeout(mut self, timeout: Duration) -> Self {
        self.authorization_timeout = timeout;
        self
    }

    /// Check if a timestamp is expired (considering the buffer).
    fn is_expired(&self, expires_at: Option<chrono::DateTime<Utc>>) -> bool {
        match expires_at {
            Some(exp) => {
                let buffer = chrono::Duration::from_std(self.expiry_buffer)
                    .unwrap_or(chrono::Duration::seconds(60));
                Utc::now() + buffer >= exp
            }
            None => false, // No expiry = never expires (FR-022)
        }
    }

}

impl CredentialResolver for DefaultCredentialResolver {
    fn resolve(&self, key: &str) -> CredentialFuture<'_, ResolvedCredential> {
        let key = key.to_string();
        Box::pin(async move {
            // Check for in-flight refresh for this key
            let existing = {
                let guard = self.in_flight.lock().await;
                guard.get(&key).cloned()
            };

            if let Some(shared_future) = existing {
                debug!(credential_key = %key, "joining existing credential resolution");
                return shared_future
                    .await
                    .map_err(|reason| CredentialError::RefreshFailed { key: key.clone(), reason });
            }

            // First, do a quick check — if the credential is valid (no refresh needed),
            // return immediately without deduplication overhead.
            let credential = self.store.get(&key).await?;
            match &credential {
                Some(Credential::ApiKey { key: api_key }) => {
                    debug!(credential_key = %key, "resolved API key credential (fast path)");
                    return Ok(ResolvedCredential::ApiKey(api_key.clone()));
                }
                Some(Credential::Bearer { token, expires_at }) if !self.is_expired(*expires_at) => {
                    debug!(credential_key = %key, "resolved bearer token credential (fast path)");
                    return Ok(ResolvedCredential::Bearer(token.clone()));
                }
                Some(Credential::OAuth2 { access_token, expires_at, .. }) if !self.is_expired(*expires_at) => {
                    debug!(credential_key = %key, "resolved OAuth2 credential (fast path)");
                    return Ok(ResolvedCredential::OAuth2AccessToken(access_token.clone()));
                }
                Some(Credential::Bearer { .. }) => {
                    // Bearer token expired — no refresh possible
                    return Err(CredentialError::Expired { key });
                }
                None => {
                    // Credential not found
                    return Err(CredentialError::NotFound { key });
                }
                _ => {
                    // OAuth2 needs refresh — use deduplication path
                }
            }

            // Slow path: credential needs refresh or doesn't exist.
            // Use deduplication to ensure only one refresh runs at a time per key.
            let shared_future = {
                let mut guard = self.in_flight.lock().await;
                // Double-check: another task might have inserted while we waited
                if let Some(existing) = guard.get(&key) {
                    existing.clone()
                } else {
                    // Create a new shared future for this refresh
                    let store = Arc::clone(&self.store);
                    let client = self.client.clone();
                    let expiry_buffer = self.expiry_buffer;
                    let authorization_handler = self.authorization_handler.clone();
                    let key_clone = key.clone();

                    let fut: BoxFuture<'static, Result<ResolvedCredential, String>> = Box::pin(async move {
                        // Re-create a temporary resolver-like context for the inner resolve
                        let resolver = InnerResolver {
                            store: &store,
                            client: &client,
                            expiry_buffer,
                            authorization_handler: authorization_handler.as_deref(),
                        };
                        resolver.resolve_inner(&key_clone).await.map_err(|e| e.to_string())
                    });

                    let shared = fut.shared();
                    guard.insert(key.clone(), shared.clone());
                    shared
                }
            };

            let result = shared_future.await;

            // Clean up the in-flight entry
            {
                let mut guard = self.in_flight.lock().await;
                guard.remove(&key);
            }

            result.map_err(|reason| CredentialError::RefreshFailed { key, reason })
        })
    }
}

/// Inner resolver context for use in the shared future (avoids lifetime issues).
struct InnerResolver<'a> {
    store: &'a Arc<dyn CredentialStore>,
    client: &'a reqwest::Client,
    expiry_buffer: Duration,
    authorization_handler: Option<&'a dyn AuthorizationHandler>,
}

impl InnerResolver<'_> {
    fn is_expired(&self, expires_at: Option<chrono::DateTime<Utc>>) -> bool {
        match expires_at {
            Some(exp) => {
                let buffer = chrono::Duration::from_std(self.expiry_buffer)
                    .unwrap_or(chrono::Duration::seconds(60));
                Utc::now() + buffer >= exp
            }
            None => false,
        }
    }

    async fn resolve_inner(&self, key: &str) -> Result<ResolvedCredential, CredentialError> {
        let credential = self.store.get(key).await?;

        match credential {
            Some(Credential::ApiKey { key: api_key }) => {
                Ok(ResolvedCredential::ApiKey(api_key))
            }

            Some(Credential::Bearer { token, expires_at }) => {
                if self.is_expired(expires_at) {
                    return Err(CredentialError::Expired { key: key.to_string() });
                }
                Ok(ResolvedCredential::Bearer(token))
            }

            Some(Credential::OAuth2 {
                access_token,
                refresh_token,
                expires_at,
                token_url,
                client_id,
                client_secret,
                scopes: _,
            }) => {
                if !self.is_expired(expires_at) {
                    return Ok(ResolvedCredential::OAuth2AccessToken(access_token));
                }

                if let Some(ref rt) = refresh_token {
                    info!(credential_key = key, "refreshing expired OAuth2 token");
                    let mut response = oauth2::refresh_token(
                        self.client,
                        &token_url,
                        rt,
                        &client_id,
                        client_secret.as_deref(),
                    )
                    .await
                    .map_err(|e| match e {
                        CredentialError::RefreshFailed { reason, .. } => {
                            CredentialError::RefreshFailed { key: key.to_string(), reason }
                        }
                        other => other,
                    })?;

                    let new_expires_at = response.expires_in.map(|secs| {
                        Utc::now() + chrono::Duration::seconds(secs)
                    });
                    let new_refresh_token = response.refresh_token.take().or(Some(rt.clone()));

                    let new_credential = Credential::OAuth2 {
                        access_token: response.access_token.clone(),
                        refresh_token: new_refresh_token,
                        expires_at: new_expires_at,
                        token_url,
                        client_id,
                        client_secret,
                        scopes: vec![],
                    };

                    self.store.set(key, new_credential).await?;
                    Ok(ResolvedCredential::OAuth2AccessToken(response.access_token))
                } else if self.authorization_handler.is_some() {
                    // Expired OAuth2 with no refresh token and handler available
                    // but we don't have enough context for the auth flow here
                    Err(CredentialError::Expired { key: key.to_string() })
                } else {
                    Err(CredentialError::Expired { key: key.to_string() })
                }
            }

            None => {
                if self.authorization_handler.is_some() {
                    // Handler available but no credential and no OAuth2 context
                    Err(CredentialError::NotFound { key: key.to_string() })
                } else {
                    Err(CredentialError::NotFound { key: key.to_string() })
                }
            }
        }
    }
}
