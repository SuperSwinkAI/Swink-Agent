# AGENTS.md — swink-agent-auth

## Scope

`auth/` — Credential management and OAuth2 support. Depends only on the `swink-agent` public API.

## Key Facts

- `InMemoryCredentialStore` — thread-safe in-memory credential storage (`Arc<RwLock<HashMap>>`).
- `DefaultCredentialResolver` — credential resolution with expiry checking, OAuth2 refresh, and concurrent request deduplication via `SingleFlightTokenSource`.
- `SingleFlightTokenSource` — deduplicates concurrent token refreshes using `futures::Shared`; only one refresh HTTP call fires even under parallel requests.
- `ExpiringValue<T>` — wraps a value with a `chrono::DateTime` expiry timestamp.
- OAuth2 refresh is in `oauth2.rs`; resolver wires store + token source together.

## Lessons Learned

- `DefaultCredentialResolver` can reuse a per-key `SingleFlightTokenSource`, but the credential store remains the source of truth. Clear the token source's cached value before resolving an expired key from the store, or a previously refreshed token can mask later external store updates.
- Adapters that need token caching should depend on `SingleFlightTokenSource` from this crate rather than rolling an adapter-local `RwLock<Option<_>>` cache, which does not deduplicate concurrent refreshes.

## Build & Test

```bash
cargo build -p swink-agent-auth
cargo test -p swink-agent-auth
cargo clippy -p swink-agent-auth -- -D warnings
```
