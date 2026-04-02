# Public API Contract: 016-adapter-azure

## Re-exports (from `adapters/src/lib.rs`)

```rust
// Feature-gated: #[cfg(feature = "azure")]
pub use azure::AzureStreamFn;
pub use azure::AzureAuth;
```

## `AzureAuth` enum

```rust
#[derive(Clone)]
pub enum AzureAuth {
    /// API key authentication via `api-key` header.
    ApiKey(String),
    /// Azure AD / Entra ID via OAuth2 client credentials flow.
    EntraId {
        tenant_id: String,
        client_id: String,
        client_secret: String,
    },
}
```

## `AzureStreamFn` struct

```rust
pub struct AzureStreamFn { /* private fields */ }

impl AzureStreamFn {
    /// Create a new Azure OpenAI adapter.
    ///
    /// `base_url`: Resource endpoint including deployment path, e.g.,
    ///   `https://my-resource.openai.azure.com/openai/deployments/my-deployment`
    ///   Trailing slashes are stripped.
    ///
    /// `auth`: Authentication method (API key or Entra ID credentials).
    pub fn new(base_url: impl Into<String>, auth: AzureAuth) -> Self;
}

impl StreamFn for AzureStreamFn { ... }
impl Debug for AzureStreamFn { ... }  // Redacts credentials
// Send + Sync guaranteed via const assertion
```

## Core Crate Additions

### `StreamErrorKind` (extended)

```rust
pub enum StreamErrorKind {
    // ... existing variants ...
    /// Provider's content safety filter blocked the response.
    ContentFiltered,
}
```

### `AssistantMessageEvent` (new constructor)

```rust
impl AssistantMessageEvent {
    /// Create a content-filtered error event.
    pub fn error_content_filtered(message: impl Into<String>) -> Self;
}
```

### `AgentError` (extended)

```rust
pub enum AgentError {
    // ... existing variants ...
    /// Provider's content safety filter blocked the response (non-retryable).
    #[error("content filtered by provider safety policy")]
    ContentFiltered,
}
```
