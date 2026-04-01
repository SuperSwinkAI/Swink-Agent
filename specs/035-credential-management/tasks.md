# Tasks: OAuth2 & Credential Management

**Input**: Design documents from `/specs/035-credential-management/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/public-api.md, quickstart.md

**Tests**: Included per constitution II (Test-Driven Development is NON-NEGOTIABLE).

**Organization**: Tasks grouped by user story for independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create the `swink-agent-auth` crate scaffold and add core type stubs.

- [ ] T001 Create auth crate directory structure: `auth/Cargo.toml`, `auth/src/lib.rs`, `auth/src/in_memory.rs`, `auth/src/resolver.rs`, `auth/src/oauth2.rs`
- [ ] T002 Add `"auth"` to workspace members in root `Cargo.toml` and configure `swink-agent-auth` package with dependencies: `swink-agent` (path), `reqwest`, `chrono`, `serde`, `serde_json`, `tokio`, `futures`, `thiserror`, `tracing` (all workspace deps)
- [ ] T003 Add `#![forbid(unsafe_code)]` to `auth/src/lib.rs` per constitution VI

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types, traits, and error types in `swink-agent` that ALL user stories depend on.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

### Core Types (src/credential.rs — NEW file)

- [ ] T004 Create `src/credential.rs` with `Credential` enum (ApiKey, Bearer, OAuth2 variants) with `Serialize`/`Deserialize` via `#[serde(tag = "type")]` per data-model.md. Add `chrono` workspace dep to core crate's `Cargo.toml` for `DateTime<Utc>` in Bearer/OAuth2 variants
- [ ] T005 Add `ResolvedCredential` enum (ApiKey, Bearer, OAuth2AccessToken variants) to `src/credential.rs` — no serde needed, Debug + Clone only
- [ ] T006 [P] Add `AuthConfig` struct, `AuthScheme` enum, and `CredentialType` enum to `src/credential.rs` per contracts/public-api.md
- [ ] T007 [P] Add `CredentialError` enum with `thiserror` derives to `src/credential.rs` — variants: NotFound, Expired, RefreshFailed, TypeMismatch, AuthorizationFailed, AuthorizationTimeout, StoreError, Timeout
- [ ] T008 Add `CredentialStore` trait (async get/set/delete by string key, requires Send + Sync) to `src/credential.rs` using `Pin<Box<dyn Future>>` return types (no async_trait macro)
- [ ] T009 [P] Add `CredentialResolver` trait (async resolve method, requires Send + Sync) to `src/credential.rs`
- [ ] T010 [P] Add `AuthorizationHandler` trait (async authorize method, requires Send + Sync) to `src/credential.rs`
- [ ] T011 Add `Credential::credential_type()` helper method that returns `CredentialType` for the variant
- [ ] T012 Re-export all credential types from `src/lib.rs`: `AuthConfig`, `AuthScheme`, `AuthorizationHandler`, `Credential`, `CredentialError`, `CredentialResolver`, `CredentialStore`, `CredentialType`, `ResolvedCredential`

### AgentTool Trait Extension

- [ ] T013 Add `fn auth_config(&self) -> Option<AuthConfig>` default method returning `None` to `AgentTool` trait in `src/tool.rs`
- [ ] T014 Add `credential: Option<ResolvedCredential>` parameter to `AgentTool::execute()` signature in `src/tool.rs`
- [ ] T015 Update all built-in tool implementations (`BashTool`, `ReadFileTool`, `WriteFileTool`) in `src/builtin_tools/` to accept the new `credential` parameter (pass-through, ignored)
- [ ] T016 Update `MockTool` and all test tool implementations in `tests/common/mod.rs` and `src/testing.rs` to accept the new `credential` parameter

### Agent Integration

- [ ] T017 Add `credential_resolver: Option<Arc<dyn CredentialResolver>>` field to `AgentOptions` in `src/agent_options.rs` and add `with_credential_resolver()` builder method
- [ ] T018 Add `credential_resolver: Option<Arc<dyn CredentialResolver>>` field to `AgentLoopConfig` in `src/loop_/mod.rs` and wire it from `AgentOptions` during loop config construction

### Tool Dispatch Integration

- [ ] T019 Wire credential resolution into `dispatch_single_tool()` in `src/loop_/tool_dispatch.rs`: after schema validation, before `tool.execute()` — check `tool.auth_config()`, if Some call resolver, pass result to execute; if None pass `None`
- [ ] T020 Add type mismatch check (FR-018) in `src/loop_/tool_dispatch.rs`: compare `AuthConfig.credential_type` against resolved credential's type, return error if mismatch
- [ ] T021 Add timeout wrapper around credential resolution in `src/loop_/tool_dispatch.rs` using `tokio::time::timeout` (configurable, default 30s)
- [ ] T022 Ensure credential resolution errors produce `AgentToolResult::error()` with key name only (no secret values) and skip tool execution (FR-015, FR-016)

### Foundational Tests

- [ ] T023 [P] Write tests for `Credential` serde roundtrip (all 3 variants) in `src/credential.rs` #[cfg(test)] module
- [ ] T024 [P] Write tests for `CredentialError` Display output to verify no secret leakage in `src/credential.rs` #[cfg(test)] module
- [ ] T025 Write test that `AgentTool::auth_config()` default returns `None` in `src/tool.rs` #[cfg(test)] module
- [ ] T026 Write test in `src/loop_/tool_dispatch.rs` #[cfg(test)] that unauthenticated tools (no auth_config) receive `credential: None` and execute normally with zero overhead

**Checkpoint**: Foundation ready — all core types, traits, and dispatch integration in place. User story implementation can begin.

---

## Phase 3: User Story 1 — Tool Uses API Key Credential (Priority: P1) 🎯 MVP

**Goal**: A tool declares an API key auth requirement, the in-memory store resolves it, and the tool receives the credential.

**Independent Test**: Seed an `InMemoryCredentialStore` with an API key, create a tool with `auth_config()`, run the agent, verify the tool receives `ResolvedCredential::ApiKey`.

### Tests for User Story 1

- [ ] T027 [P] [US1] Write test for `InMemoryCredentialStore::new()` with pre-seeded credentials — get returns Some, missing returns None — in `auth/tests/in_memory_tests.rs`
- [ ] T028 [P] [US1] Write test for `InMemoryCredentialStore::set()` and `delete()` roundtrip in `auth/tests/in_memory_tests.rs`
- [ ] T029 [P] [US1] Write test for `InMemoryCredentialStore` thread safety — concurrent reads and writes from multiple tokio tasks — in `auth/tests/in_memory_tests.rs`
- [ ] T030 [US1] Write test for `DefaultCredentialResolver` resolving an API key (no expiry, no refresh) in `auth/tests/resolver_tests.rs`
- [ ] T031 [US1] Write test for resolver returning `CredentialError::NotFound` when key is missing in `auth/tests/resolver_tests.rs`
- [ ] T032 [US1] Write test for type mismatch: tool expects Bearer but store has ApiKey — verify error result with type mismatch message in `src/loop_/tool_dispatch.rs` #[cfg(test)] module (mismatch check is in dispatch per T020, not the resolver)

### Implementation for User Story 1

- [ ] T033 [US1] Implement `InMemoryCredentialStore` in `auth/src/in_memory.rs`: `new(HashMap)`, `empty()`, `with_credential()` builder, `Arc<RwLock<HashMap>>` backing, implement `CredentialStore` trait
- [ ] T034 [US1] Implement basic `DefaultCredentialResolver` in `auth/src/resolver.rs`: constructor with `Arc<dyn CredentialStore>`, `resolve()` that calls `store.get()` and converts `Credential` → `ResolvedCredential` (API key path only for now)
- [ ] T035 [US1] Re-export `InMemoryCredentialStore` and `DefaultCredentialResolver` from `auth/src/lib.rs`
- [ ] T036 [US1] Write integration test: create agent with `InMemoryCredentialStore` seeded with API key, tool with `auth_config()`, verify tool receives `ResolvedCredential::ApiKey` during execution — in `auth/tests/integration_tests.rs`

**Checkpoint**: US1 complete. API key credential resolution works end-to-end. MVP deliverable.

---

## Phase 4: User Story 2 — Bearer Token with Automatic Expiry Check (Priority: P1)

**Goal**: Bearer tokens with expiry are validated before tool execution. Expired tokens produce clear errors.

**Independent Test**: Store a bearer token with past expiry, execute the tool, verify expiry error returned without tool executing.

**Dependencies**: Requires Phase 3 (US1) for base resolver infrastructure.

### Tests for User Story 2

- [ ] T037 [P] [US2] Write test: bearer token with future expiry resolves successfully in `auth/tests/resolver_tests.rs`
- [ ] T038 [P] [US2] Write test: bearer token with past expiry returns `CredentialError::Expired` in `auth/tests/resolver_tests.rs`
- [ ] T039 [P] [US2] Write test: bearer token with no expiry (None) resolves successfully (FR-022) in `auth/tests/resolver_tests.rs`
- [ ] T040 [US2] Write test: bearer token expiring within buffer period (default 60s) is treated as expired (FR-023) in `auth/tests/resolver_tests.rs`
- [ ] T041 [US2] Write test: custom expiry buffer (e.g., 120s) is respected in `auth/tests/resolver_tests.rs`

### Implementation for User Story 2

- [ ] T042 [US2] Extend `DefaultCredentialResolver::resolve()` in `auth/src/resolver.rs` to handle Bearer variant: check `expires_at` against `Utc::now() + expiry_buffer`, return `Expired` error if within buffer
- [ ] T043 [US2] Add `with_expiry_buffer(Duration)` builder method to `DefaultCredentialResolver` in `auth/src/resolver.rs` (default: 60 seconds)

**Checkpoint**: US2 complete. Bearer token expiry validation works. Tools receive clear errors for expired tokens.

---

## Phase 5: User Story 3 — OAuth2 Token Auto-Refresh (Priority: P2)

**Goal**: Expired OAuth2 tokens are automatically refreshed via the token endpoint. Concurrent refreshes are deduplicated to exactly one HTTP request.

**Independent Test**: Store an expired OAuth2 credential with a refresh token, mock the token endpoint with `wiremock`, verify refresh occurs, store is updated, and tool receives fresh access token.

**Dependencies**: Requires Phase 4 (US2) for expiry checking infrastructure.

### Tests for User Story 3

- [ ] T044 [P] [US3] Write test: valid (non-expired) OAuth2 credential resolves to `ResolvedCredential::OAuth2AccessToken` in `auth/tests/resolver_tests.rs`
- [ ] T045 [US3] Write test: expired OAuth2 with refresh token triggers refresh POST to token_url, store updated with new tokens — use `wiremock` mock server — in `auth/tests/oauth2_tests.rs`
- [ ] T046 [US3] Write test: expired OAuth2 with no refresh token returns `CredentialError::Expired` in `auth/tests/resolver_tests.rs`
- [ ] T047 [US3] Write test: refresh request fails (HTTP 400/401) returns `CredentialError::RefreshFailed` in `auth/tests/oauth2_tests.rs`
- [ ] T048 [US3] Write test: two concurrent resolves for the same expired OAuth2 credential result in exactly one HTTP refresh request (deduplication) — use `wiremock` request count assertion — in `auth/tests/oauth2_tests.rs`
- [ ] T049 [US3] Write test: refresh for key A does not block or deduplicate with refresh for key B in `auth/tests/oauth2_tests.rs`

### Implementation for User Story 3

- [ ] T050 [US3] Implement OAuth2 token refresh helper in `auth/src/oauth2.rs`: POST to `token_url` with `grant_type=refresh_token`, `refresh_token`, `client_id`, optional `client_secret`; parse JSON response for `access_token`, `refresh_token`, `expires_in`
- [ ] T051 [US3] Extend `DefaultCredentialResolver` in `auth/src/resolver.rs` to handle OAuth2 variant: check expiry (with buffer), if expired and has refresh_token call refresh helper, update store with new credential via `store.set()`
- [ ] T052 [US3] Add `with_client(reqwest::Client)` builder method to `DefaultCredentialResolver` in `auth/src/resolver.rs` (default: new Client)
- [ ] T053 [US3] Implement concurrent refresh deduplication in `auth/src/resolver.rs`: add `tokio::sync::Mutex<HashMap<String, Shared<BoxFuture>>>` field, insert before refresh, await shared future for concurrent requests, remove entry on completion

**Checkpoint**: US3 complete. OAuth2 auto-refresh works with deduplication. Core credential lifecycle is fully functional.

---

## Phase 6: User Story 4 — OAuth2 Initial Authorization Flow (Priority: P2)

**Goal**: When no credential exists for a key and an authorization handler is configured, the framework initiates the OAuth2 authorization code flow, exchanges the code for tokens, and stores them.

**Independent Test**: Configure a mock authorization handler, attempt to resolve a missing credential, verify the handler is called with the authorization URL and tokens are stored after exchange.

**Dependencies**: Requires Phase 5 (US3) for OAuth2 refresh infrastructure (code exchange reuses the token endpoint POST pattern).

### Tests for User Story 4

- [ ] T054 [P] [US4] Write test: missing credential with authorization handler triggers handler callback with correct authorization URL in `auth/tests/oauth2_tests.rs`
- [ ] T055 [US4] Write test: authorization handler returns code, code exchanged for tokens, tokens stored in credential store in `auth/tests/oauth2_tests.rs`
- [ ] T056 [US4] Write test: missing credential with no authorization handler returns `CredentialError::NotFound` (FR-011) in `auth/tests/resolver_tests.rs`
- [ ] T057 [US4] Write test: authorization handler returns error → `CredentialError::AuthorizationFailed` in `auth/tests/oauth2_tests.rs`
- [ ] T058 [US4] Write test: authorization flow exceeds timeout → `CredentialError::AuthorizationTimeout` (FR-020, default 5 min) in `auth/tests/oauth2_tests.rs`

### Implementation for User Story 4

- [ ] T059 [US4] Add `with_authorization_handler(Arc<dyn AuthorizationHandler>)` builder method to `DefaultCredentialResolver` in `auth/src/resolver.rs`
- [ ] T060 [US4] Implement authorization code exchange in `auth/src/oauth2.rs`: POST to `token_url` with `grant_type=authorization_code`, `code`, `client_id`, optional `client_secret`, `redirect_uri`; parse token response
- [ ] T061 [US4] Extend `DefaultCredentialResolver::resolve()` in `auth/src/resolver.rs`: when `store.get()` returns `None` and handler is configured, build authorization URL (with state CSRF token, scopes, client_id), call `handler.authorize()`, exchange code for tokens, call `store.set()`, return resolved credential
- [ ] T062 [US4] Add `with_timeout(Duration)` builder method to `DefaultCredentialResolver` in `auth/src/resolver.rs` for resolution timeout (default: 30s) and separate authorization timeout (default: 5 min per FR-020)

**Checkpoint**: US4 complete. Full OAuth2 lifecycle works: initial authorization → token storage → auto-refresh.

---

## Phase 7: User Story 5 — Headless Deployment with Pre-Provisioned Credentials (Priority: P3)

**Goal**: Agents in headless environments work with pre-provisioned credentials. No interactive authorization is attempted when no handler is configured.

**Independent Test**: Create agent with pre-provisioned OAuth2 credentials and no authorization handler. Verify tools execute using stored credentials and refresh works. Verify missing credentials produce errors (no interactive fallback).

**Dependencies**: Requires Phase 5 (US3) for refresh. US4 authorization handler must be optional.

### Tests for User Story 5

- [ ] T063 [P] [US5] Write test: pre-provisioned OAuth2 credentials resolve without authorization handler in `auth/tests/resolver_tests.rs`
- [ ] T064 [P] [US5] Write test: pre-provisioned expired OAuth2 with refresh token auto-refreshes without authorization handler in `auth/tests/oauth2_tests.rs`
- [ ] T065 [US5] Write test: expired credential with no refresh token and no authorization handler returns `CredentialError::Expired` (not AuthorizationFailed) in `auth/tests/resolver_tests.rs`

### Implementation for User Story 5

- [ ] T066 [US5] Verify `DefaultCredentialResolver` correctly skips authorization flow when `authorization_handler` is `None` in `auth/src/resolver.rs` (should already work from US4 implementation — this task validates the path and adds any missing guard)

**Checkpoint**: US5 complete. Headless deployments verified.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Final cleanup, secret leakage verification, workspace integration.

- [ ] T067 [P] Add `Debug` implementation for `InMemoryCredentialStore` that does NOT print credential values (only key count) in `auth/src/in_memory.rs`
- [ ] T068 [P] Write test that `tracing` output from credential resolution contains no secret values — enable tracing subscriber in test, resolve a credential, assert log output contains key name but not token value — in `auth/tests/resolver_tests.rs`
- [ ] T069 [P] Run `cargo clippy --workspace -- -D warnings` and fix any warnings across all modified files
- [ ] T070 [P] Run `cargo test --workspace` to verify no regressions in existing tests (especially tools that now have the new `credential` parameter)
- [ ] T071 Update `CLAUDE.md` Active Technologies section with 035 entry: Rust 1.88 + swink-agent-auth crate dependencies and in-memory storage
- [ ] T072 Verify `cargo test -p swink-agent --no-default-features` still passes (builtin-tools disabled path)
- [ ] T073 Run quickstart.md examples as validation: verify code patterns compile and work as documented

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Phase 2 — MVP target
- **US2 (Phase 4)**: Depends on Phase 3 (uses resolver infrastructure)
- **US3 (Phase 5)**: Depends on Phase 4 (uses expiry checking)
- **US4 (Phase 6)**: Depends on Phase 5 (reuses token endpoint POST pattern)
- **US5 (Phase 7)**: Depends on Phase 5 (validates no-handler path)
- **Polish (Phase 8)**: Depends on all user stories complete

### User Story Dependencies

- **US1 (P1)**: Foundation only — no other story deps. **MVP**.
- **US2 (P1)**: Builds on US1's resolver. Tests are independent.
- **US3 (P2)**: Builds on US2's expiry checking. Adds HTTP refresh.
- **US4 (P2)**: Builds on US3's OAuth2 infrastructure. Adds authorization flow.
- **US5 (P3)**: Validates US3's no-handler path. Minimal new code.

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Types/models before services
- Services before integration
- Story complete before moving to next priority

### Parallel Opportunities

- T006 + T007: AuthConfig/AuthScheme and CredentialError are independent types
- T009 + T010: CredentialResolver and AuthorizationHandler traits are independent
- T023 + T024 + T025 + T026: Foundational tests touch different modules
- T027 + T028 + T029: InMemoryCredentialStore tests are independent
- T037 + T038 + T039: Bearer expiry tests are independent scenarios
- T044 (valid OAuth2) can parallel with other US3 tests
- T054 can parallel with other US4 tests
- T063 + T064: US5 tests are independent scenarios
- T067 + T068 + T069 + T070: Polish tasks touch different concerns

---

## Parallel Example: User Story 1

```bash
# Launch all InMemoryCredentialStore tests together:
T027: "InMemoryCredentialStore::new() with pre-seeded credentials"
T028: "InMemoryCredentialStore::set() and delete() roundtrip"
T029: "InMemoryCredentialStore thread safety"

# Then sequentially:
T033: "Implement InMemoryCredentialStore"
T034: "Implement basic DefaultCredentialResolver"
T035: "Re-export from auth/src/lib.rs"
T036: "Integration test: agent with API key tool"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001-T003)
2. Complete Phase 2: Foundational (T004-T026) — CRITICAL
3. Complete Phase 3: User Story 1 (T027-T036)
4. **STOP and VALIDATE**: API key credential resolution works end-to-end
5. The agent can resolve API keys for tools — immediate value

### Incremental Delivery

1. Setup + Foundational → Types and dispatch wiring ready
2. Add US1 → API key resolution works → **MVP**
3. Add US2 → Bearer expiry validation works
4. Add US3 → OAuth2 auto-refresh works → **Major milestone**
5. Add US4 → Full OAuth2 lifecycle → **Feature complete**
6. Add US5 → Headless validated → **Production ready**
7. Polish → Ship it

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story
- Breaking change: `AgentTool::execute()` gains `credential` parameter — all tool impls must update (T014-T016)
- The `auth` crate follows the same pattern as `policies` crate: depends only on `swink-agent` public API
- Constitution II requires tests before implementation — each US phase has tests first
- FR-016 (secrets never logged) must be verified in T068 and T022
