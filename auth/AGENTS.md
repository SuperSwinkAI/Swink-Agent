# AGENTS.md — swink-agent-auth

## Scope

`auth/` — Credential management and OAuth2 support.

## Key Invariants

- `InMemoryCredentialStore` — `Arc<RwLock<HashMap>>`.
- `DefaultCredentialResolver` — expiry checking, OAuth2 refresh, deduplication via `SingleFlightTokenSource`.
- Clear `SingleFlightTokenSource` cached value before resolving expired key from store (prevents stale token masking).
- Adapters needing token cache should use `SingleFlightTokenSource`, not ad-hoc `RwLock<Option<_>>`.
- OAuth2 refresh failures surface only `HTTP status + error code` — never raw `error_description` or token-endpoint bodies.
- OAuth2 logging redacts endpoints to scheme/host; transport failures collapse to stable reason codes.
