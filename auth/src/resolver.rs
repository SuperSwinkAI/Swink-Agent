//! Default credential resolver with expiry checking, OAuth2 refresh, and
//! concurrent request deduplication.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{PoisonError, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tracing::{debug, info};

use swink_agent::{
    AuthorizationHandler, Credential, CredentialError, CredentialFuture, CredentialResolver,
    CredentialStore, ResolvedCredential,
};

use crate::oauth2::AuthorizationConfig;
use crate::{ExpiringValue, SingleFlightTokenSource, oauth2};

/// Default resolution timeout (FR-014): bounds the non-interactive
/// resolution path (store lookups and OAuth2 refresh).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default authorization timeout (FR-020): bounds how long the interactive
/// authorization flow may take to complete. This is intentionally separate
/// from and longer than [`DEFAULT_TIMEOUT`] — waiting on a human to complete
/// a browser flow is expected to take much longer than a store lookup or
/// token refresh.
const DEFAULT_AUTHORIZATION_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Default credential resolver that handles:
/// - API key passthrough
/// - Bearer token expiry validation
/// - OAuth2 token refresh with deduplication
/// - OAuth2 interactive authorization for credentials with no stored value
pub struct DefaultCredentialResolver {
    store: Arc<dyn CredentialStore>,
    client: reqwest::Client,
    expiry_buffer: Duration,
    /// Per-key token sources used to share a single refresh (or a single
    /// interactive authorization attempt) across concurrent resolutions
    /// without keeping a second bespoke in-flight map here.
    refresh_sources:
        RwLock<HashMap<String, Arc<SingleFlightTokenSource<ResolvedCredential, CredentialError>>>>,
    authorization_handler: Option<Arc<dyn AuthorizationHandler>>,
    /// Per-key `OAuth2` client configuration needed to build an
    /// authorization URL for a credential that has no stored value yet. A
    /// key with no entry here behaves as if no authorization handler were
    /// configured, even if `authorization_handler` is `Some`.
    authorization_configs: HashMap<String, AuthorizationConfig>,
    /// Bounds the non-interactive resolution path (FR-014, default 30s).
    timeout: Duration,
    /// Bounds the interactive authorization path (FR-020, default 5m).
    authorization_timeout: Duration,
}

impl DefaultCredentialResolver {
    /// Create a resolver backed by the given credential store.
    #[must_use]
    pub fn new(store: Arc<dyn CredentialStore>) -> Self {
        Self {
            store,
            client: reqwest::Client::new(),
            expiry_buffer: Duration::from_secs(60),
            refresh_sources: RwLock::new(HashMap::new()),
            authorization_handler: None,
            authorization_configs: HashMap::new(),
            timeout: DEFAULT_TIMEOUT,
            authorization_timeout: DEFAULT_AUTHORIZATION_TIMEOUT,
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

    /// Set the authorization handler used to initiate interactive `OAuth2`
    /// authorization code flows (FR-010) for credential keys that have no
    /// stored value.
    #[must_use]
    pub fn with_authorization_handler(mut self, handler: Arc<dyn AuthorizationHandler>) -> Self {
        self.authorization_handler = Some(handler);
        self
    }

    /// Register the `OAuth2` client configuration used to build an
    /// authorization URL for `key` when it has no stored credential.
    ///
    /// Required (alongside [`with_authorization_handler`](Self::with_authorization_handler))
    /// for the interactive authorization flow to trigger for `key`; without
    /// it, a missing credential for `key` resolves to
    /// [`CredentialError::NotFound`] exactly as if no handler were
    /// configured (FR-011).
    #[must_use]
    pub fn with_authorization_config(
        mut self,
        key: impl Into<String>,
        config: AuthorizationConfig,
    ) -> Self {
        self.authorization_configs.insert(key.into(), config);
        self
    }

    /// Set the resolution timeout (default: 30 seconds, FR-014).
    ///
    /// Bounds the non-interactive resolution path: store lookups and
    /// `OAuth2` refresh. This does NOT bound the interactive authorization
    /// flow — see [`with_authorization_timeout`](Self::with_authorization_timeout)
    /// for that (FR-020), which defaults to a much longer 5 minutes since it
    /// waits on a human to complete a browser flow.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the authorization timeout (default: 5 minutes, FR-020).
    ///
    /// Bounds how long the interactive authorization flow (handler
    /// invocation plus code-for-token exchange) may take to complete before
    /// resolution fails with [`CredentialError::AuthorizationTimeout`].
    #[must_use]
    pub const fn with_authorization_timeout(mut self, timeout: Duration) -> Self {
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

    fn refresh_source(
        &self,
        key: &str,
    ) -> Arc<SingleFlightTokenSource<ResolvedCredential, CredentialError>> {
        if let Some(existing) = self
            .refresh_sources
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
        {
            return Arc::clone(existing);
        }

        let mut sources = self
            .refresh_sources
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        Arc::clone(
            sources
                .entry(key.to_string())
                .or_insert_with(|| Arc::new(SingleFlightTokenSource::new(self.expiry_buffer))),
        )
    }
}

impl CredentialResolver for DefaultCredentialResolver {
    fn resolve(&self, key: &str) -> CredentialFuture<'_, ResolvedCredential> {
        let key = key.to_string();
        Box::pin(async move {
            let stored_result =
                match tokio::time::timeout(self.timeout, self.resolve_stored(&key)).await {
                    Ok(result) => result,
                    Err(_elapsed) => return Err(CredentialError::Timeout { key }),
                };

            match stored_result {
                Err(CredentialError::NotFound { key: not_found_key }) => {
                    self.resolve_via_authorization(not_found_key).await
                }
                other => other,
            }
        })
    }
}

impl DefaultCredentialResolver {
    /// Resolve `key` against the store only: fast paths for `ApiKey` and
    /// valid `Bearer`/`OAuth2` credentials, `OAuth2` refresh (deduplicated),
    /// and `CredentialError::NotFound`/`Expired` for the remaining cases.
    /// Does NOT attempt interactive authorization — that is layered on top
    /// by [`resolve_via_authorization`](Self::resolve_via_authorization).
    async fn resolve_stored(&self, key: &str) -> Result<ResolvedCredential, CredentialError> {
        let key = key.to_string();
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
            Some(Credential::OAuth2 {
                access_token,
                expires_at,
                ..
            }) if !self.is_expired(*expires_at) => {
                debug!(credential_key = %key, "resolved OAuth2 credential (fast path)");
                return Ok(ResolvedCredential::OAuth2AccessToken(access_token.clone()));
            }
            Some(Credential::Bearer { .. }) => {
                return Err(CredentialError::Expired { key });
            }
            None => {
                return Err(CredentialError::NotFound { key });
            }
            Some(Credential::OAuth2 { refresh_token, .. }) => {
                if refresh_token.is_none() {
                    return Err(CredentialError::Expired { key });
                }
            }
        }

        let source = self.refresh_source(&key);
        source.clear_cached();
        debug!(
            credential_key = %key,
            "resolving refreshable OAuth2 credential via single-flight token source"
        );

        let store = Arc::clone(&self.store);
        let client = self.client.clone();
        let expiry_buffer = self.expiry_buffer;
        let refresh_key = key.clone();

        source
            .get_or_refresh(move || async move {
                let resolver = InnerResolver {
                    store,
                    client,
                    expiry_buffer,
                };
                resolver.resolve_refreshable(&refresh_key).await
            })
            .await
    }

    /// Look up the registered per-key authorization inputs; returns `None`
    /// if either the handler or the key's `AuthorizationConfig` is missing,
    /// in which case the caller falls back to `NotFound` (FR-011).
    fn authorization_inputs(
        &self,
        key: &str,
    ) -> Option<(Arc<dyn AuthorizationHandler>, AuthorizationConfig)> {
        let handler = self.authorization_handler.as_ref()?;
        let config = self.authorization_configs.get(key)?;
        Some((Arc::clone(handler), config.clone()))
    }

    /// Handle a `NotFound` result from [`resolve_stored`](Self::resolve_stored)
    /// by attempting the interactive `OAuth2` authorization flow (US4), if
    /// configured for `key`. Concurrent calls for the same `key` are
    /// deduplicated to a single handler invocation via the same
    /// single-flight infrastructure used for token refresh.
    async fn resolve_via_authorization(
        &self,
        key: String,
    ) -> Result<ResolvedCredential, CredentialError> {
        let Some((handler, config)) = self.authorization_inputs(&key) else {
            return Err(CredentialError::NotFound { key });
        };

        let source = self.refresh_source(&key);
        // Unlike the refresh path (which always follows a real store read
        // just above), a key can only reach this method via `NotFound`, so a
        // stale cached value here would be surprising. Clear defensively.
        source.clear_cached();
        let store = Arc::clone(&self.store);
        let client = self.client.clone();
        let authorization_timeout = self.authorization_timeout;
        let auth_key = key.clone();

        source
            .get_or_refresh(move || async move {
                let outcome = tokio::time::timeout(
                    authorization_timeout,
                    perform_authorization(&handler, &store, &client, &auth_key, &config),
                )
                .await;

                match outcome {
                    Ok(Ok(resolved)) => Ok(ExpiringValue::new(
                        resolved,
                        InnerResolver::cache_deadline(None),
                    )),
                    Ok(Err(error)) => Err(error),
                    Err(_elapsed) => Err(CredentialError::AuthorizationTimeout { key: auth_key }),
                }
            })
            .await
    }
}

/// Run the interactive authorization handler and exchange the resulting code
/// for tokens, storing the new credential on success.
///
/// Errors surfaced from `exchange_code` use an empty `key` placeholder (see
/// [`oauth2::exchange_code`]); this fills it in. Errors returned directly by
/// the handler are propagated unchanged — the handler is responsible for
/// constructing a meaningful `CredentialError`.
async fn perform_authorization(
    handler: &Arc<dyn AuthorizationHandler>,
    store: &Arc<dyn CredentialStore>,
    client: &reqwest::Client,
    key: &str,
    config: &AuthorizationConfig,
) -> Result<ResolvedCredential, CredentialError> {
    let state = uuid::Uuid::new_v4().to_string();
    let auth_url =
        oauth2::build_authorization_url(config, &state).map_err(|error| match error {
            CredentialError::AuthorizationFailed { reason, .. } => {
                CredentialError::AuthorizationFailed {
                    key: key.to_string(),
                    reason,
                }
            }
            other => other,
        })?;

    info!(credential_key = %key, "initiating interactive OAuth2 authorization");
    let code = handler.authorize(&auth_url, &state).await?;

    let mut response = oauth2::exchange_code(
        client,
        &config.token_url,
        &code,
        &config.client_id,
        config.client_secret.as_deref(),
        &config.redirect_uri,
    )
    .await
    .map_err(|error| match error {
        CredentialError::AuthorizationFailed { reason, .. } => {
            CredentialError::AuthorizationFailed {
                key: key.to_string(),
                reason,
            }
        }
        other => other,
    })?;

    let expires_at = response
        .expires_in
        .map(|secs| Utc::now() + chrono::Duration::seconds(secs));
    let new_credential = Credential::OAuth2 {
        access_token: response.access_token.clone(),
        refresh_token: response.refresh_token.take(),
        expires_at,
        token_url: config.token_url.clone(),
        client_id: config.client_id.clone(),
        client_secret: config.client_secret.clone(),
        scopes: config.scopes.clone(),
    };

    store.set(key, new_credential).await?;
    info!(credential_key = %key, "interactive OAuth2 authorization completed; credential stored");

    Ok(ResolvedCredential::OAuth2AccessToken(response.access_token))
}

/// Inner resolver context for use in the shared future.
struct InnerResolver {
    store: Arc<dyn CredentialStore>,
    client: reqwest::Client,
    expiry_buffer: Duration,
}

impl InnerResolver {
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

    fn cache_deadline(expires_at: Option<DateTime<Utc>>) -> Instant {
        match expires_at {
            Some(exp) => match (exp - Utc::now()).to_std() {
                Ok(remaining) => Instant::now() + remaining,
                Err(_) => Instant::now(),
            },
            None => Instant::now() + Duration::from_secs(60 * 60 * 24 * 365),
        }
    }

    async fn resolve_refreshable(
        &self,
        key: &str,
    ) -> Result<ExpiringValue<ResolvedCredential>, CredentialError> {
        let credential = self.store.get(key).await?;

        match credential {
            Some(Credential::ApiKey { key: api_key }) => Ok(ExpiringValue::new(
                ResolvedCredential::ApiKey(api_key),
                Self::cache_deadline(None),
            )),

            Some(Credential::Bearer { token, expires_at }) => {
                if self.is_expired(expires_at) {
                    return Err(CredentialError::Expired {
                        key: key.to_string(),
                    });
                }
                Ok(ExpiringValue::new(
                    ResolvedCredential::Bearer(token),
                    Self::cache_deadline(expires_at),
                ))
            }

            Some(Credential::OAuth2 {
                access_token,
                refresh_token,
                expires_at,
                token_url,
                client_id,
                client_secret,
                scopes,
            }) => {
                if !self.is_expired(expires_at) {
                    return Ok(ExpiringValue::new(
                        ResolvedCredential::OAuth2AccessToken(access_token),
                        Self::cache_deadline(expires_at),
                    ));
                }

                if let Some(ref rt) = refresh_token {
                    info!(credential_key = key, "refreshing expired OAuth2 token");
                    let mut response = oauth2::refresh_token(
                        &self.client,
                        &token_url,
                        rt,
                        &client_id,
                        client_secret.as_deref(),
                    )
                    .await
                    .map_err(|e| match e {
                        CredentialError::RefreshFailed { reason, .. } => {
                            CredentialError::RefreshFailed {
                                key: key.to_string(),
                                reason,
                            }
                        }
                        other => other,
                    })?;

                    let new_expires_at = response
                        .expires_in
                        .map(|secs| Utc::now() + chrono::Duration::seconds(secs));
                    let new_refresh_token = response.refresh_token.take().or(Some(rt.clone()));

                    let new_credential = Credential::OAuth2 {
                        access_token: response.access_token.clone(),
                        refresh_token: new_refresh_token,
                        expires_at: new_expires_at,
                        token_url,
                        client_id,
                        client_secret,
                        scopes,
                    };

                    self.store.set(key, new_credential).await?;
                    Ok(ExpiringValue::new(
                        ResolvedCredential::OAuth2AccessToken(response.access_token),
                        Self::cache_deadline(new_expires_at),
                    ))
                } else {
                    Err(CredentialError::Expired {
                        key: key.to_string(),
                    })
                }
            }

            None => Err(CredentialError::NotFound {
                key: key.to_string(),
            }),
        }
    }
}
