//! Default credential resolver with expiry checking, OAuth2 refresh, and
//! concurrent request deduplication.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{PoisonError, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tracing::{debug, info};

use swink_agent::{
    Credential, CredentialError, CredentialFuture, CredentialResolver, CredentialStore,
    ResolvedCredential,
};

use crate::{ExpiringValue, SingleFlightTokenSource, oauth2};

/// Default credential resolver that handles:
/// - API key passthrough
/// - Bearer token expiry validation
/// - OAuth2 token refresh with deduplication
pub struct DefaultCredentialResolver {
    store: Arc<dyn CredentialStore>,
    client: reqwest::Client,
    expiry_buffer: Duration,
    /// Per-key token sources used to share a single refresh across concurrent
    /// OAuth2 resolutions without keeping a second bespoke in-flight map here.
    refresh_sources:
        RwLock<HashMap<String, Arc<SingleFlightTokenSource<ResolvedCredential, CredentialError>>>>,
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
        })
    }
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
