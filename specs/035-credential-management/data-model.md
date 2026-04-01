# Data Model: OAuth2 & Credential Management

**Feature**: 035-credential-management
**Date**: 2026-03-31

## Entities

### Credential (core crate)

A secret value with type information for tool authentication.

| Field | Type | Description |
|-------|------|-------------|
| — | enum variant | Discriminated by type |

**Variants:**

| Variant | Fields | Description |
|---------|--------|-------------|
| `ApiKey` | `key: String` | A single secret string |
| `Bearer` | `token: String`, `expires_at: Option<DateTime<Utc>>` | Token with optional expiry |
| `OAuth2` | `access_token: String`, `refresh_token: Option<String>`, `expires_at: Option<DateTime<Utc>>`, `token_url: String`, `client_id: String`, `client_secret: Option<String>`, `scopes: Vec<String>` | Full OAuth2 token set |

**Serialization**: `serde::Serialize + Deserialize` with `#[serde(tag = "type")]` for tagged enum representation.

**Validation rules**:
- `ApiKey.key` must be non-empty.
- `OAuth2.token_url` must be a valid URL.
- `OAuth2.client_id` must be non-empty.
- `OAuth2.scopes` may be empty (some providers don't require scopes for refresh).

### ResolvedCredential (core crate)

The minimal secret value delivered to a tool after resolution. Contains only what the tool needs to make an authenticated request.

| Variant | Fields | Description |
|---------|--------|-------------|
| `ApiKey` | `key: String` | Resolved API key |
| `Bearer` | `token: String` | Resolved bearer token (original or refreshed) |
| `OAuth2AccessToken` | `token: String` | Refreshed/valid OAuth2 access token |

**Note**: Does NOT contain refresh tokens, client secrets, or token URLs. These are internal to the resolver.

### AuthConfig (core crate)

Per-tool declaration of authentication requirements.

| Field | Type | Description |
|-------|------|-------------|
| `credential_key` | `String` | Key to look up in the credential store |
| `auth_scheme` | `AuthScheme` | How to attach the credential to the request |
| `credential_type` | `CredentialType` | Expected credential type (for mismatch check) |

### AuthScheme (core crate)

How the resolved credential is attached to the outbound request.

| Variant | Fields | Description |
|---------|--------|-------------|
| `BearerHeader` | — | `Authorization: Bearer {token}` |
| `ApiKeyHeader` | `header_name: String` | `{header_name}: {key}` |
| `ApiKeyQuery` | `param_name: String` | `?{param_name}={key}` |

### CredentialType (core crate)

Enum for type mismatch checking (FR-018).

| Variant | Description |
|---------|-------------|
| `ApiKey` | Expects an API key credential |
| `Bearer` | Expects a bearer token |
| `OAuth2` | Expects an OAuth2 token set |

### CredentialStore (trait, core crate)

Pluggable storage abstraction.

| Method | Signature | Description |
|--------|-----------|-------------|
| `get` | `async fn get(&self, key: &str) -> Result<Option<Credential>, CredentialError>` | Retrieve by key |
| `set` | `async fn set(&self, key: &str, credential: Credential) -> Result<(), CredentialError>` | Store/update by key |
| `delete` | `async fn delete(&self, key: &str) -> Result<(), CredentialError>` | Remove by key |

**Thread-safety**: Trait requires `Send + Sync`.

### CredentialResolver (trait, core crate)

Orchestrator for credential resolution.

| Method | Signature | Description |
|--------|-----------|-------------|
| `resolve` | `async fn resolve(&self, key: &str) -> Result<ResolvedCredential, CredentialError>` | Resolve a credential by key (check expiry, refresh if needed) |

**Thread-safety**: Trait requires `Send + Sync`.

### AuthorizationHandler (trait, core crate)

Callback for interactive OAuth2 authorization.

| Method | Signature | Description |
|--------|-----------|-------------|
| `authorize` | `async fn authorize(&self, auth_url: &str, state: &str) -> Result<String, CredentialError>` | Present auth URL to user, return authorization code |

### CredentialError (core crate)

| Variant | Description |
|---------|-------------|
| `NotFound` | Credential key not in store |
| `Expired` | Token expired and no refresh available |
| `RefreshFailed` | OAuth2 refresh request failed |
| `TypeMismatch` | Stored credential type doesn't match tool expectation |
| `AuthorizationFailed` | Interactive authorization flow failed |
| `AuthorizationTimeout` | User didn't complete authorization in time |
| `StoreError` | Underlying store error (wraps `Box<dyn Error>`) |
| `Timeout` | Resolution exceeded configured timeout |

### InMemoryCredentialStore (auth crate)

Concrete implementation of `CredentialStore`.

| Field | Type | Description |
|-------|------|-------------|
| `credentials` | `Arc<RwLock<HashMap<String, Credential>>>` | Thread-safe credential map |

**Seeding**: Constructed with initial credentials via `new(HashMap<String, Credential>)` or builder pattern.

### DefaultCredentialResolver (auth crate)

Concrete implementation of `CredentialResolver`.

| Field | Type | Description |
|-------|------|-------------|
| `store` | `Arc<dyn CredentialStore>` | Backing store |
| `client` | `reqwest::Client` | HTTP client for OAuth2 refresh |
| `authorization_handler` | `Option<Arc<dyn AuthorizationHandler>>` | Optional interactive auth |
| `expiry_buffer` | `Duration` | Proactive refresh window (default: 60s) |
| `in_flight` | `tokio::sync::Mutex<HashMap<String, Shared<BoxFuture<...>>>>` | Deduplication map |
| `timeout` | `Duration` | Resolution timeout (default: 30s) |

## State Transitions

### Credential Lifecycle

```
[Not Stored] --seed at config--> [Valid]
[Valid] --time passes--> [Expiring (within buffer)]
[Expiring] --auto refresh--> [Valid] (new tokens written to store)
[Expiring] --refresh fails--> [Error]
[Valid] --time passes past expiry--> [Expired]
[Expired] --auto refresh--> [Valid]
[Expired] --refresh fails--> [Error]
[Not Stored] --authorization flow--> [Valid] (tokens written to store)
[Not Stored] --no handler configured--> [Error]
```

### Resolution Flow

```
resolve(key)
  ├── store.get(key)
  │   ├── None → check authorization handler
  │   │   ├── handler configured → authorize() → store.set() → resolve again
  │   │   └── no handler → CredentialError::NotFound
  │   └── Some(credential)
  │       ├── ApiKey → ResolvedCredential::ApiKey (always valid)
  │       ├── Bearer
  │       │   ├── no expiry → ResolvedCredential::Bearer
  │       │   ├── valid → ResolvedCredential::Bearer
  │       │   └── expired/expiring → CredentialError::Expired
  │       └── OAuth2
  │           ├── valid → ResolvedCredential::OAuth2AccessToken
  │           └── expired/expiring
  │               ├── has refresh_token → refresh (deduplicated) → store.set() → ResolvedCredential::OAuth2AccessToken
  │               └── no refresh_token → CredentialError::Expired
  └── timeout wraps entire flow
```

## Relationships

```
AgentOptions --configures--> CredentialResolver (optional)
AgentLoopConfig --holds--> CredentialResolver (optional)
CredentialResolver --uses--> CredentialStore
CredentialResolver --uses--> AuthorizationHandler (optional)
CredentialResolver --uses--> reqwest::Client (for refresh)
AgentTool --declares--> AuthConfig (optional, via auth_config())
tool_dispatch --calls--> CredentialResolver.resolve()
tool_dispatch --passes--> ResolvedCredential to tool.execute()
InMemoryCredentialStore --implements--> CredentialStore
DefaultCredentialResolver --implements--> CredentialResolver
```
