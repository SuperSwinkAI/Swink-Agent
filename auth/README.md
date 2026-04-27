# swink-agent-auth

[![Crates.io](https://img.shields.io/crates/v/swink-agent-auth.svg)](https://crates.io/crates/swink-agent-auth)
[![Docs.rs](https://docs.rs/swink-agent-auth/badge.svg)](https://docs.rs/swink-agent-auth)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

Credential management and OAuth2 token refresh for [`swink-agent`](https://crates.io/crates/swink-agent) — plug-in auth for provider adapters that need signed or short-lived tokens.

## Features

- **`DefaultCredentialResolver`** — resolves credentials on demand with expiry checks, proactive OAuth2 refresh, and request deduplication
- **`InMemoryCredentialStore`** — thread-safe `Arc`-shareable store for tests and short-lived processes
- **`SingleFlightTokenSource`** — concurrent refresh coalescing: N callers waiting on an expiring token trigger exactly one refresh
- OAuth2 helpers (`oauth2` module) — client-credentials and refresh-token grants over `reqwest`
- Integrates with `swink-agent-adapters` (`azure` feature) to inject AAD bearer tokens into every request

## Quick Start

```toml
[dependencies]
swink-agent = "0.10.0"
swink-agent-auth = "0.10.0"
tokio = { version = "1", features = ["full"] }
```

```rust
use std::sync::Arc;
use swink_agent::CredentialStore;
use swink_agent_auth::{DefaultCredentialResolver, InMemoryCredentialStore};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::empty());
    let resolver = Arc::new(DefaultCredentialResolver::new(store));

    // Hand `resolver` to AgentOptions::with_credential_resolver so adapters
    // can fetch/refresh tokens automatically during a run.
    Ok(())
}
```

## Architecture

Credentials are keyed by provider-defined identifiers (`CredentialKey`) and returned as opaque `CredentialValue`s. `DefaultCredentialResolver` wraps a pluggable `CredentialStore` with an expiry-aware cache and a `SingleFlightTokenSource` that collapses concurrent refresh attempts into one network call — preventing thundering-herd problems when a token expires under load.

No `unsafe` code (`#![forbid(unsafe_code)]`). Credential values are held in memory; the crate does not itself persist secrets to disk.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
