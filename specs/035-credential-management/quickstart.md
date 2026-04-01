# Quickstart: OAuth2 & Credential Management

**Feature**: 035-credential-management
**Date**: 2026-03-31

## Basic Usage

### Tool with API Key Authentication

```rust
use swink_agent::{Agent, AgentOptions, AgentTool, AgentToolResult, AuthConfig, AuthScheme, CredentialType, ResolvedCredential};
use swink_agent_auth::{InMemoryCredentialStore, DefaultCredentialResolver};
use std::sync::Arc;

struct WeatherTool;

impl AgentTool for WeatherTool {
    fn name(&self) -> &str { "get_weather" }
    fn label(&self) -> &str { "Get Weather" }
    fn description(&self) -> &str { "Fetch current weather for a city" }
    fn parameters_schema(&self) -> &Value { /* ... */ }

    fn auth_config(&self) -> Option<AuthConfig> {
        Some(AuthConfig {
            credential_key: "weather-api".into(),
            auth_scheme: AuthScheme::ApiKeyHeader("X-API-Key".into()),
            credential_type: CredentialType::ApiKey,
        })
    }

    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        credential: Option<ResolvedCredential>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async move {
            let ResolvedCredential::ApiKey(api_key) = credential.unwrap() else {
                return AgentToolResult::error("Expected API key credential");
            };

            // Use api_key in HTTP request header
            let resp = reqwest::Client::new()
                .get("https://api.weather.example.com/current")
                .header("X-API-Key", &api_key)
                .query(&[("city", params["city"].as_str().unwrap_or("London"))])
                .send()
                .await;

            // ... handle response ...
            AgentToolResult::text("Sunny, 72°F")
        })
    }
}

// Configure agent with credentials
let store = InMemoryCredentialStore::new(HashMap::from([
    ("weather-api".into(), Credential::ApiKey { key: "sk-weather-123".into() }),
]));

let resolver = DefaultCredentialResolver::new(Arc::new(store));

let options = AgentOptions::new(/* ... */)
    .with_tools(vec![Arc::new(WeatherTool)])
    .with_credential_resolver(Arc::new(resolver));

let mut agent = Agent::new(options);
agent.prompt_async("What's the weather in Tokyo?").await?;
```

### OAuth2 with Automatic Refresh

```rust
use swink_agent::{Credential, AuthConfig, AuthScheme, CredentialType};
use swink_agent_auth::{InMemoryCredentialStore, DefaultCredentialResolver};
use chrono::Utc;

// Seed store with OAuth2 tokens (e.g., from a previous authorization)
let store = InMemoryCredentialStore::new(HashMap::from([
    ("google-calendar".into(), Credential::OAuth2 {
        access_token: "ya29.expired-token".into(),
        refresh_token: Some("1//refresh-token".into()),
        expires_at: Some(Utc::now() - chrono::Duration::minutes(5)), // already expired
        token_url: "https://oauth2.googleapis.com/token".into(),
        client_id: "your-client-id.apps.googleusercontent.com".into(),
        client_secret: Some("your-client-secret".into()),
        scopes: vec!["https://www.googleapis.com/auth/calendar.readonly".into()],
    }),
]));

let resolver = DefaultCredentialResolver::new(Arc::new(store))
    .with_expiry_buffer(Duration::from_secs(120)); // refresh 2 min before expiry

let options = AgentOptions::new(/* ... */)
    .with_tools(vec![Arc::new(CalendarTool)])
    .with_credential_resolver(Arc::new(resolver));

// When CalendarTool executes, the resolver will:
// 1. Detect the access token is expired
// 2. POST to token_url with the refresh token
// 3. Update the in-memory store with new tokens
// 4. Pass the fresh access token to the tool
```

### Tool Without Authentication

```rust
struct CalculatorTool;

impl AgentTool for CalculatorTool {
    fn name(&self) -> &str { "calculate" }
    // ... other methods ...

    // auth_config() uses default (returns None) — no credential resolution happens
    // credential parameter in execute() will be None

    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        credential: Option<ResolvedCredential>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async move {
            // credential is None — zero overhead, no resolution occurred
            let result = params["a"].as_f64().unwrap() + params["b"].as_f64().unwrap();
            AgentToolResult::text(format!("{}", result))
        })
    }
}
```

### Interactive OAuth2 Authorization

```rust
use swink_agent::{AuthorizationHandler, CredentialError};

struct BrowserAuthHandler;

impl AuthorizationHandler for BrowserAuthHandler {
    fn authorize(
        &self,
        auth_url: &str,
        _state: &str,
    ) -> Pin<Box<dyn Future<Output = Result<String, CredentialError>> + Send + '_>> {
        Box::pin(async move {
            // Open browser and start local callback server
            open::that(auth_url).ok();
            // Listen for redirect on localhost...
            let code = listen_for_callback(8080).await?;
            Ok(code)
        })
    }
}

let resolver = DefaultCredentialResolver::new(Arc::new(store))
    .with_authorization_handler(Arc::new(BrowserAuthHandler));
```

### Headless Deployment (No Interactive Auth)

```rust
// Pre-provisioned credentials, no authorization handler
let store = InMemoryCredentialStore::new(pre_provisioned_credentials);
let resolver = DefaultCredentialResolver::new(Arc::new(store));
// No .with_authorization_handler() — if credentials are missing,
// the resolver returns CredentialError::NotFound instead of attempting
// interactive authorization.
```

## Key Points

- **Zero overhead**: Tools without `auth_config()` skip credential resolution entirely
- **Type-safe delivery**: Tools receive `Option<ResolvedCredential>` — pattern match on the variant
- **Automatic refresh**: OAuth2 tokens refreshed transparently; tools only see fresh access tokens
- **Deduplication**: Concurrent tools needing the same expired credential trigger only one refresh
- **Secrets never logged**: `CredentialError` messages include key names, never secret values
- **In-memory only**: Built-in store is in-memory, seeded at config time. Custom stores implement the trait
- **Breaking change**: `AgentTool::execute` gains a `credential` parameter — all tool implementations must update
