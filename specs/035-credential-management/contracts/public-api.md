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
        credential: Option<ResolvedCredential>,  // NEW parameter
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>>;
}
```

## AgentOptions Extension (swink-agent crate)

```rust
impl AgentOptions {
    /// Configure a credential resolver for tool authentication.
    pub fn with_credential_resolver(self, resolver: Arc<dyn CredentialResolver>) -> Self;
}
```

## AgentLoopConfig Extension (swink-agent crate)

```rust
pub struct AgentLoopConfig {
    // ... existing fields ...
    pub credential_resolver: Option<Arc<dyn CredentialResolver>>,  // NEW field
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

    /// Set the expiry buffer (default: 60 seconds).
    pub fn with_expiry_buffer(self, buffer: Duration) -> Self;

    /// Set the resolution timeout (default: 30 seconds).
    pub fn with_timeout(self, timeout: Duration) -> Self;
}

impl CredentialResolver for DefaultCredentialResolver { /* ... */ }
```

## Re-exports (swink-agent crate lib.rs)

```rust
pub use credential::{
    AuthConfig, AuthScheme, AuthorizationHandler, Credential, CredentialError,
    CredentialResolver, CredentialStore, CredentialType, ResolvedCredential,
};
```
