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

## Cargo features

| Feature | Default | Provides |
|---|---|---|
| `keychain` | off | `KeychainCredentialStore` — persists credentials in the OS keychain |

### `keychain`

The in-memory store loses everything on exit, so an OAuth2 refresh token obtained
in one run is gone by the next. Enable `keychain` to persist credentials in the
platform secret store instead — macOS Keychain Services, Windows Credential
Manager, or Linux/BSD Secret Service:

```toml
swink-agent-auth = { version = "0.11.0", features = ["keychain"] }
```

```rust,no_run
use std::sync::Arc;
use swink_agent::{Credential, CredentialStore};
use swink_agent_auth::{DefaultCredentialResolver, KeychainCredentialStore};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let store = KeychainCredentialStore::new();
    store
        .set("github", Credential::ApiKey { key: "ghp_example".into() })
        .await?;

    // Survives a restart, so a refreshed OAuth2 token is still usable next run.
    let resolver = Arc::new(DefaultCredentialResolver::new(Arc::new(store)));
    Ok(())
}
```

Notes:

- **Opt-in only.** The framework never reads the keychain on its own — you
  construct the store and pass it in. A default build does not depend on
  `keyring` at all.
- **Namespacing.** Entries are written under the `swink-agent-auth` service name.
  Use `KeychainCredentialStore::with_service("my-app")` to isolate an
  application's credentials from other `swink-agent` processes on the machine.
- **Headless hosts.** A machine with no unlocked keyring (many CI containers, some
  Linux servers) yields `CredentialError::StoreError`. Keep the in-memory store
  for those deployments.
- **Testing.** Substitute the `KeychainBackend` trait to test against a fake
  instead of the real OS keychain.
- **Blocking I/O.** Keychain calls can block — for example, macOS may prompt the
  user — so every operation runs on `tokio::task::spawn_blocking`. A Tokio
  runtime must be active.

## Quick Start

```toml
[dependencies]
swink-agent = "0.9.0"
swink-agent-auth = "0.9.0"
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
