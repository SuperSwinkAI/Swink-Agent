# Research: OAuth2 & Credential Management

**Feature**: 035-credential-management
**Date**: 2026-03-31

## R1: Crate Boundary — Core Types vs Auth Crate

**Decision**: Define types (`Credential`, `AuthConfig`, `ResolvedCredential`), the `CredentialStore` trait, and the `AuthorizationHandler` trait in the core `swink-agent` crate. Implement `InMemoryCredentialStore`, `CredentialResolver`, and OAuth2 refresh logic in the new `swink-agent-auth` crate.

**Rationale**: The `AgentTool` trait lives in core and its `auth_config()` method returns `AuthConfig`, so that type must be in core. Similarly, `execute()` receives `Option<ResolvedCredential>`, so that type must be in core too. The `CredentialStore` trait has no external dependencies (just `async_trait`-style futures + serde_json), so it can live in core without pulling in HTTP deps. The `CredentialResolver` trait is defined in core (one async method), but the default implementation lives in `swink-agent-auth` because it needs `reqwest` for OAuth2 refresh.

**Alternatives considered**:
- **Everything in auth crate**: Would require core to depend on auth (circular) or use dynamic dispatch with no compile-time type safety for `AuthConfig`. Rejected.
- **Everything in core**: Pulls `reqwest` into core for OAuth2 refresh, violating constitution I (core free of provider-specific deps). Rejected.
- **Separate types crate**: Over-engineering for a handful of types. Rejected.

## R2: OAuth2 Token Refresh — oauth2 Crate vs Raw reqwest

**Decision**: Use raw `reqwest` for OAuth2 token refresh. Do not add the `oauth2` crate.

**Rationale**: The only OAuth2 operation the resolver performs is token refresh (POST to token endpoint with `grant_type=refresh_token`). This is a single HTTP POST with form-encoded body and JSON response — roughly 20 lines of reqwest code. The `oauth2` crate adds type safety for the full OAuth2 flow but brings in `url`, `http`, and its own error types for a single POST. The authorization code exchange (US4) is slightly more complex but still a single POST. The `reqwest` dependency already exists in the workspace. The auth handler abstraction already owns the browser/callback complexity.

**Alternatives considered**:
- **`oauth2` crate**: Well-maintained, 15M downloads. Adds ~4 transitive deps. Type-safe token responses. But overkill for two HTTP POSTs and adds learning curve for contributors. Rejected for initial implementation; can be reconsidered if more grant types are added.
- **`openidconnect` crate**: Even heavier. Rejected.

## R3: Concurrent Refresh Deduplication

**Decision**: Use `tokio::sync::Mutex<HashMap<String, Shared<BoxFuture>>>` pattern. When a refresh is in-flight for a key, subsequent requests `.await` the shared future instead of issuing a new HTTP request.

**Rationale**: The `futures::future::Shared` combinator clones the output to all waiters. Combined with a mutex-guarded map of in-flight refreshes, this provides exactly-once execution with fan-out. The map entry is inserted before the refresh starts and removed on completion. This is a standard pattern for request coalescing in async Rust.

**Alternatives considered**:
- **`tokio::sync::watch`**: Requires a known initial value and is channel-oriented (sender/receiver). Less natural for one-shot request coalescing. Rejected.
- **`tokio::sync::OnceCell` per key**: Would need a map of OnceCells, essentially the same pattern but less flexible (can't reset after error). Rejected.
- **`singleflight` crate**: Third-party crate that does exactly this. Only ~50k downloads, small maintenance surface. Implementing the pattern ourselves is ~30 lines and avoids the dependency. Rejected per constitution IV (crate doesn't pass the 80% threshold — we'd use 100% of its API, but the implementation is trivial).

## R4: CredentialResolver Trait Design

**Decision**: Define `CredentialResolver` as an async trait in core with a single method: `async fn resolve(&self, key: &str) -> Result<ResolvedCredential, CredentialError>`. The resolver handles expiry checking, refresh, and deduplication internally.

**Rationale**: A single `resolve()` method keeps the interface minimal. The resolver receives the `CredentialStore` at construction time, not per-call. Type mismatch checking happens in the tool dispatch layer (comparing `AuthConfig.credential_type` against the resolved credential's type), not inside the resolver — the resolver just returns whatever the store has.

**Alternatives considered**:
- **Two methods (check + refresh)**: Exposes internals. Callers would need to orchestrate check-then-refresh, duplicating logic. Rejected.
- **Resolver as a function**: Loses the ability to hold state (deduplication map, reqwest client). Rejected.
- **No trait, concrete type only**: Would prevent consumers from providing custom resolution logic (e.g., vault integration). Rejected.

## R5: Tool Dispatch Integration Point

**Decision**: Inject credential resolution into `dispatch_single_tool()` in `tool_dispatch.rs`, between schema validation and `tool.execute()`. The resolver is accessed via `AgentLoopConfig` (new optional field).

**Rationale**: The dispatch pipeline currently runs: Pre-dispatch policies → Approval → Schema validation → Execute. Credential resolution fits naturally after validation (arguments are valid) and before execution (tool needs the credential). The resolver is async, which is compatible with the spawned task context. If no resolver is configured (FR-019), the code path is a simple `None` check — zero overhead.

**Alternatives considered**:
- **Pre-dispatch policy**: Policies return verdicts, not data. A credential is data the tool needs, not a continue/skip decision. Rejected.
- **Before schema validation**: No reason to resolve credentials for tools that will fail validation. Rejected.
- **Inside tool.execute()**: Violates the spec requirement that tools don't manage credential lookup. Rejected.

## R6: ResolvedCredential Design

**Decision**: `ResolvedCredential` is an enum mirroring credential types but containing only the secret value needed for the request, not the full OAuth2 metadata. Variants: `ApiKey(String)`, `Bearer(String)`, `OAuth2AccessToken(String)`.

**Rationale**: Tools don't need refresh tokens, client IDs, or token endpoints. They need the resolved secret to attach to their HTTP request. The `AuthConfig` on the tool specifies *how* to attach it (bearer header, API key header, query param). Keeping `ResolvedCredential` minimal avoids exposing internal OAuth2 state to tools.

**Alternatives considered**:
- **Pass full `Credential` enum**: Leaks refresh tokens and client secrets to tools. Security risk per FR-016. Rejected.
- **Just a `String`**: Loses type information needed for FR-018 (mismatch check). Rejected.
- **Opaque wrapper with `as_str()` only**: Prevents pattern matching. Less ergonomic for tools that need to know the credential type. Rejected.

## R7: New Crate Justification (Constitution Compliance)

**Decision**: Create `swink-agent-auth` as the 8th workspace member.

**Rationale**: Constitution says "Adding a crate requires justification that no existing crate boundary can absorb the concern." The credential resolver needs `reqwest` for OAuth2 refresh. Core (`swink-agent`) must remain free of provider-specific and HTTP dependencies (constitution I). The adapters crate is for LLM provider adapters, not tool-level auth. The policies crate handles policy verdicts, not credential resolution. The memory crate handles session persistence, not secrets. No existing crate boundary fits. This follows the exact precedent of `swink-agent-policies` (created to keep policy implementations out of core).

**Alternatives considered**:
- **Module in core with feature gate**: Feature-gated `reqwest` in core still adds the dep to the core crate's dependency graph, even if optional. Violates constitution I spirit. Rejected.
- **Module in adapters**: Adapters are LLM-specific. Tool auth is a cross-cutting concern. Would create a confusing dependency where core depends on adapters. Rejected.
