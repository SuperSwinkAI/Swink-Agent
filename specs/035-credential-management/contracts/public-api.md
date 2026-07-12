# Public API Contract: OAuth2 & Credential Management

**Feature**: 035-credential-management
**Date**: 2026-03-31

## Core Types (swink-agent crate)

### Credential

```rust
/// A secret value with type information for tool authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Credential {
    ApiKey {
        key: String,
    },
    Bearer {
        token: String,
        #[serde(default)]
        expires_at: Option<DateTime<Utc>>,
    },
    OAuth2 {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<DateTime<Utc>>,
        token_url: String,
        client_id: String,
        client_secret: Option<String>,
        #[serde(default)]
        scopes: Vec<String>,
    },
}
```

### ResolvedCredential

```rust
/// Minimal secret value delivered to a tool after credential resolution.
/// Does NOT contain refresh tokens, client secrets, or token endpoints.
#[derive(Debug, Clone)]
pub enum ResolvedCredential {
    ApiKey(String),
    Bearer(String),
    OAuth2AccessToken(String),
}
```

### AuthConfig

```rust
/// Per-tool declaration of authentication requirements.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Key to look up in the credential store.
    pub credential_key: String,
    /// How to attach the credential to the outbound request.
    pub auth_scheme: AuthScheme,
    /// Expected credential type (for mismatch checking).
    pub credential_type: CredentialType,
}
```

### AuthScheme

```rust
/// How a resolved credential is attached to the outbound request.
#[derive(Debug, Clone)]
pub enum AuthScheme {
    /// Authorization: Bearer {token}
    BearerHeader,
    /// {header_name}: {key}
    ApiKeyHeader(String),
    /// ?{param_name}={key}
    ApiKeyQuery(String),
}
```

### CredentialType

```rust
/// Credential type discriminant for mismatch checking (FR-018).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialType {
    ApiKey,
    Bearer,
    OAuth2,
}
```

### CredentialError

```rust
/// Errors from credential resolution.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("credential not found: {key}")]
    NotFound { key: String },

    #[error("credential expired: {key}")]
    Expired { key: String },

    #[error("credential refresh failed for {key}: {reason}")]
    RefreshFailed { key: String, reason: String },

    #[error("credential type mismatch for {key}: expected {expected:?}, got {actual:?}")]
    TypeMismatch { key: String, expected: CredentialType, actual: CredentialType },

    #[error("authorization failed for {key}: {reason}")]
    AuthorizationFailed { key: String, reason: String },

    #[error("authorization timed out for {key}")]
    AuthorizationTimeout { key: String },

    #[error("credential store error: {0}")]
    StoreError(Box<dyn std::error::Error + Send + Sync>),

    #[error("credential resolution timed out for {key}")]
    Timeout { key: String },
}
```

### CredentialStore Trait

```rust
/// Pluggable credential storage abstraction.
/// Thread-safe for concurrent tool executions.
pub trait CredentialStore: Send + Sync {
    /// Retrieve a credential by key.
    fn get(
        &self,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Credential>, CredentialError>> + Send + '_>>;

    /// Store or update a credential by key.
    fn set(
        &self,
        key: &str,
        credential: Credential,
    ) -> Pin<Box<dyn Future<Output = Result<(), CredentialError>> + Send + '_>>;

    /// Delete a credential by key.
    fn delete(
        &self,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), CredentialError>> + Send + '_>>;
}
```

### CredentialResolver Trait

```rust
/// Orchestrator for credential resolution — checks validity, triggers
/// refresh, deduplicates concurrent requests.
pub trait CredentialResolver: Send + Sync {
    /// Resolve a credential by key. Returns the minimal secret value
    /// needed for the authenticated request.
    fn resolve(
        &self,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedCredential, CredentialError>> + Send + '_>>;
}
```

### AuthorizationHandler Trait

```rust
/// Callback for interactive OAuth2 authorization code flows.
/// The handler receives the authorization URL and returns the authorization code.
pub trait AuthorizationHandler: Send + Sync {
    /// Present the authorization URL to the user and return the authorization code.
    /// `state` is the CSRF token for verification.
    fn authorize(
        &self,
        auth_url: &str,
        state: &str,
    ) -> Pin<Box<dyn Future<Output = Result<String, CredentialError>> + Send + '_>>;
}
```

## AgentTool Trait Extension (swink-agent crate)

### New Default Method

```rust
pub trait AgentTool: Send + Sync {
    // ... existing methods unchanged ...

    /// Optional authentication configuration. Default: no auth required.
    fn auth_config(&self) -> Option<AuthConfig> {
        None
    }

    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        state: Arc<std::sync::RwLock<SessionState>>,  // added by spec 034
        credential: Option<ResolvedCredential>,  // NEW parameter (this spec)
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>>;
}
```

Note: the `state` parameter (session key-value state, spec 034) was added between
`on_update` and `credential` — the real signature in `src/tool.rs` has six
parameters, not five. This doc previously omitted `state` entirely.

## AgentOptions Extension (swink-agent crate)

```rust
impl AgentOptions {
    /// Configure a credential resolver for tool authentication.
    pub fn with_credential_resolver(self, resolver: Arc<dyn CredentialResolver>) -> Self;

    /// Set the timeout applied around credential resolution (default: 30 seconds).
    ///
    /// This satisfies FR-014 ("configurable timeout"). It wraps every dispatch-layer
    /// call to `CredentialResolver::resolve()` in `tokio::time::timeout`, independent
    /// of any internal timeout a resolver implementation applies on its own network
    /// calls. A resolve that does not complete in time surfaces
    /// `CredentialError::Timeout` instead of executing the tool.
    pub const fn with_credential_timeout(self, timeout: Duration) -> Self;
}
```

## AgentLoopConfig Extension (swink-agent crate)

```rust
pub struct AgentLoopConfig {
    // ... existing fields ...
    pub credential_resolver: Option<Arc<dyn CredentialResolver>>,  // NEW field
    pub credential_timeout: Duration,  // NEW field, defaults to 30 seconds
}
```

## Auth Crate Types (swink-agent-auth crate)

### InMemoryCredentialStore

```rust
/// Thread-safe in-memory credential store. Seeded at construction.
#[derive(Debug, Clone)]
pub struct InMemoryCredentialStore { /* ... */ }

impl InMemoryCredentialStore {
    /// Create a store pre-populated with credentials.
    pub fn new(credentials: HashMap<String, Credential>) -> Self;

    /// Create an empty store.
    pub fn empty() -> Self;

    /// Builder: add a single credential.
    pub fn with_credential(self, key: impl Into<String>, credential: Credential) -> Self;
}

impl CredentialStore for InMemoryCredentialStore { /* ... */ }
```

### DefaultCredentialResolver

```rust
/// Default credential resolver with OAuth2 refresh and deduplication.
pub struct DefaultCredentialResolver { /* ... */ }

impl DefaultCredentialResolver {
    /// Create a resolver with the given store.
    pub fn new(store: Arc<dyn CredentialStore>) -> Self;

    /// Set the HTTP client for OAuth2 refresh (default: new reqwest::Client).
    pub fn with_client(self, client: reqwest::Client) -> Self;

    /// Set the authorization handler for interactive flows.
    pub fn with_authorization_handler(self, handler: Arc<dyn AuthorizationHandler>) -> Self;

    /// Register the OAuth2 client configuration (authorization endpoint, token
    /// endpoint, client id/secret, redirect URI, scopes) needed to build an
    /// authorization URL for `key` when it has no stored credential.
    ///
    /// IMPLEMENTATION NOTE (added 2026-07-06, deviates from the original
    /// contract draft): the original US4 flow diagram ("None → handler
    /// configured → authorize() → store.set()") did not specify where the
    /// OAuth2 client id, token endpoint, redirect URI, and scopes come from
    /// for a key that has *no* stored credential yet — `Credential::OAuth2`
    /// only exists once a credential has been issued. `AuthorizationConfig`
    /// (in `swink-agent-auth`, re-exported at the crate root) fills that gap:
    /// a key must have both a handler and a registered `AuthorizationConfig`
    /// for the interactive flow to trigger. A handler configured without a
    /// matching `AuthorizationConfig` for `key` behaves exactly as if no
    /// handler were configured (FR-011: `NotFound`).
    pub fn with_authorization_config(self, key: impl Into<String>, config: AuthorizationConfig) -> Self;

    /// Set the expiry buffer (default: 60 seconds).
    pub fn with_expiry_buffer(self, buffer: Duration) -> Self;

    /// Set the resolution timeout (default: 30 seconds, FR-014).
    ///
    /// IMPLEMENTED 2026-07-06 in `auth/src/resolver.rs`. Bounds the
    /// non-interactive resolution path (store lookups and OAuth2 refresh),
    /// independent of the dispatch-layer `AgentOptions::with_credential_timeout`
    /// (see above), which wraps every `resolve()` call from outside the
    /// resolver regardless of implementation. This resolver-level timeout
    /// additionally bounds the resolver's own internal work (e.g. the OAuth2
    /// refresh HTTP call).
    ///
    /// Deviation from the original contract draft: this timeout does NOT
    /// bound the interactive authorization flow — see
    /// `with_authorization_timeout` below. A 30-second default is far too
    /// short for a human to complete a browser-based authorization, so the
    /// two are intentionally separate knobs.
    pub const fn with_timeout(self, timeout: Duration) -> Self;

    /// Set the authorization timeout (default: 5 minutes, FR-020).
    ///
    /// NEW in this pass (not in the original contract draft, added to make
    /// FR-020 configurable as required). Bounds how long the interactive
    /// authorization flow (handler invocation plus code-for-token exchange)
    /// may take before resolution fails with
    /// `CredentialError::AuthorizationTimeout`.
    pub const fn with_authorization_timeout(self, timeout: Duration) -> Self;
}

impl CredentialResolver for DefaultCredentialResolver { /* ... */ }
```

### AuthorizationConfig (auth crate)

```rust
/// OAuth2 client configuration needed to construct an authorization URL and
/// exchange the resulting code for tokens, for a credential key that has no
/// stored credential yet (US4: initial authorization flow).
///
/// NEW in this pass — see the IMPLEMENTATION NOTE under
/// `DefaultCredentialResolver::with_authorization_config` above for why this
/// type exists.
#[derive(Debug, Clone)]
pub struct AuthorizationConfig {
    pub authorization_endpoint: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

/// Build the authorization URL for the given config and CSRF `state` token.
pub fn build_authorization_url(config: &AuthorizationConfig, state: &str) -> Result<String, CredentialError>;

/// Exchange an OAuth2 authorization code for tokens (T060).
pub async fn exchange_code(
    client: &reqwest::Client,
    token_url: &str,
    code: &str,
    client_id: &str,
    client_secret: Option<&str>,
    redirect_uri: &str,
) -> Result<TokenResponse, CredentialError>;
```

## Re-exports (swink-agent crate lib.rs)

```rust
pub use credential::{
    AuthConfig, AuthScheme, AuthorizationHandler, Credential, CredentialError,
    CredentialResolver, CredentialStore, CredentialType, ResolvedCredential,
};
```
