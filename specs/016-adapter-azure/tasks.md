# Tasks: Adapter: Azure OpenAI

**Input**: Design documents from `/specs/016-adapter-azure/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup

**Purpose**: No new project setup needed — adapter crate and feature gate already exist. This phase covers preparatory verification.

- [x] T001 Verify `azure` feature flag compiles cleanly with `cargo check -p swink-agent-adapters --features azure`
- [x] T002 Verify existing `adapters/src/azure.rs` stub builds and existing tests pass with `cargo test -p swink-agent-adapters`

---

## Phase 2: Foundational (Core Crate — ContentFiltered Error)

**Purpose**: Add `ContentFiltered` error support to the core crate. MUST complete before any user story work — adapters depend on these types.

**CRITICAL**: No user story work can begin until this phase is complete.

- [x] T003 Add `ContentFiltered` variant to `StreamErrorKind` enum in `src/stream.rs`
- [x] T004 Add `error_content_filtered(message)` constructor to `AssistantMessageEvent` in `src/stream.rs`
- [x] T005 Add unit test for `error_content_filtered` constructor in `src/stream.rs` (follows existing `error_throttled`/`error_auth` test pattern)
- [x] T006 Add `ContentFiltered` variant to `AgentError` enum in `src/error.rs` with `#[error("content filtered by provider safety policy")]`
- [x] T007 Verify `AgentError::ContentFiltered` is non-retryable — add test in `src/error.rs` asserting `is_retryable()` returns false
- [x] T008 Map `StreamErrorKind::ContentFiltered` to `AgentError::ContentFiltered` in agent loop error handling in `src/loop_.rs`
- [x] T009 Re-export `ContentFiltered` variant — verify it's accessible via `swink_agent::error::AgentError::ContentFiltered` and `swink_agent::stream::StreamErrorKind::ContentFiltered`
- [x] T010 Run `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` to verify no regressions

**Checkpoint**: Core crate now supports ContentFiltered errors — adapter work can begin.

---

## Phase 3: User Story 1 — Stream Text Responses (Priority: P1) MVP

**Goal**: Stream text responses from Azure OpenAI v1 GA endpoint with API key auth.

**Independent Test**: Send a prompt to an Azure deployment, verify text deltas arrive incrementally and assembled message is coherent.

### Implementation for User Story 1

- [x] T011 [US1] Define `AzureAuth` enum with `ApiKey(String)` and `EntraId { tenant_id, client_id, client_secret }` variants in `adapters/src/azure.rs`
- [x] T012 [US1] Implement `Clone` for `AzureAuth` and `Debug` that redacts secrets in `adapters/src/azure.rs`
- [x] T013 [US1] Update `AzureStreamFn` struct to hold `auth: AzureAuth` and `token_cache: Arc<RwLock<Option<CachedToken>>>` in `adapters/src/azure.rs`
- [x] T014 [US1] Define internal `CachedToken` struct with `access_token: String` and `expires_at: Instant` in `adapters/src/azure.rs`
- [x] T015 [US1] Update `AzureStreamFn::new()` constructor to accept `(base_url, AzureAuth)` instead of `(base_url, api_key)` in `adapters/src/azure.rs`
- [x] T016 [US1] Update `Debug` impl for `AzureStreamFn` to redact all credential fields in `adapters/src/azure.rs`
- [x] T017 [US1] Update `send_request` to select auth header based on `AzureAuth` variant — `api-key` header for `ApiKey`, `Authorization: Bearer` for `EntraId` in `adapters/src/azure.rs`
- [x] T018 [US1] Re-export `AzureAuth` from `adapters/src/lib.rs` under `#[cfg(feature = "azure")]`
- [x] T019 [US1] Add wiremock test: text streaming with API key auth — verify SSE text deltas arrive incrementally in `adapters/tests/azure.rs`
- [x] T020 [US1] Add wiremock test: verify `api-key` header is set correctly on requests in `adapters/tests/azure.rs`
- [x] T021 [US1] Add wiremock test: verify trailing slash in base URL is stripped in `adapters/tests/azure.rs`
- [x] T022 [US1] Add wiremock test: verify `[DONE]` sentinel produces terminal Done event in `adapters/tests/azure.rs`
- [x] T023 [US1] Run `cargo test -p swink-agent-adapters --features azure` to verify all US1 tests pass

**Checkpoint**: Text streaming with API key auth works end-to-end. MVP complete.

---

## Phase 4: User Story 2 — Stream Tool Call Responses (Priority: P1)

**Goal**: Stream tool call chunks with names, IDs, and incremental JSON arguments from Azure.

**Independent Test**: Send a prompt with tool definitions, verify tool call events have correct names, IDs, and parseable arguments.

### Implementation for User Story 2

- [x] T024 [US2] Add wiremock test: tool call streaming — verify ToolCallStart, ToolCallDelta, ToolCallEnd events with correct tool name and ID in `adapters/tests/azure.rs`
- [x] T025 [US2] Add wiremock test: multiple parallel tool calls — verify each emitted as separate indexed block in `adapters/tests/azure.rs`
- [x] T026 [US2] Add wiremock test: tool call arguments form valid JSON upon ToolCallEnd in `adapters/tests/azure.rs`
- [x] T027 [US2] Run `cargo test -p swink-agent-adapters --features azure` to verify all US2 tests pass

**Checkpoint**: Tool call streaming works. No adapter code changes needed — `openai_compat` handles tool calls already.

---

## Phase 5: User Story 3 — Deployment Routing & Azure AD Auth (Priority: P2)

**Goal**: Construct correct v1 GA URLs from resource endpoint and deployment name. Support Azure AD/Entra ID OAuth2 client credentials flow.

**Independent Test**: Verify URL construction is correct. Verify Entra ID tokens are acquired, cached, and refreshed.

### Implementation for User Story 3

- [x] T028 [US3] Implement `acquire_token` async function — POST to `https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token` with client credentials in `adapters/src/azure.rs`
- [x] T029 [US3] Implement `get_or_refresh_token` — check cache, acquire if empty/expired, cache result with `expires_at` derived from `expires_in` response field in `adapters/src/azure.rs`
- [x] T030 [US3] Add refresh margin constant (e.g., 300 seconds) — tokens refresh proactively before expiry in `adapters/src/azure.rs`
- [x] T031 [US3] Update `send_request` to call `get_or_refresh_token` when auth is `EntraId` and set `Authorization: Bearer {token}` header in `adapters/src/azure.rs`
- [x] T032 [US3] Add wiremock test: Entra ID token acquisition — mock token endpoint, verify POST params (grant_type, client_id, client_secret, scope) in `adapters/tests/azure.rs`
- [x] T033 [US3] Add wiremock test: token caching — two requests reuse same token (token endpoint called once) in `adapters/tests/azure.rs`
- [x] T034 [US3] Add wiremock test: token refresh — expired token triggers re-acquisition in `adapters/tests/azure.rs`
- [x] T035 [US3] Add wiremock test: Bearer token appears in Authorization header on API requests in `adapters/tests/azure.rs`
- [x] T036 [US3] Add wiremock test: URL constructed as `{base_url}/chat/completions` with deployment in base_url path in `adapters/tests/azure.rs`
- [x] T037 [US3] Run `cargo test -p swink-agent-adapters --features azure` to verify all US3 tests pass

**Checkpoint**: Azure AD auth and deployment routing work end-to-end.

---

## Phase 6: User Story 4 — Error Handling & Content Filtering (Priority: P2)

**Goal**: Classify Azure HTTP errors correctly. Detect content filter violations and surface as `ContentFiltered`.

**Independent Test**: Simulate error responses (429, 401, 404, 500, content filter) and verify correct error type mapping.

### Implementation for User Story 4

- [ ] T038 [US4] Add content filter detection in Azure stream — check `finish_reason: "content_filter"` in SSE chunks and emit `error_content_filtered` event in `adapters/src/azure.rs`
- [ ] T039 [US4] Add Azure error body parsing — detect `error.code: "ContentFilterBlocked"` in HTTP error responses and emit `error_content_filtered` in `adapters/src/azure.rs`
- [ ] T040 [US4] Add wiremock test: HTTP 429 → rate-limit error (retryable) with retry-after in `adapters/tests/azure.rs`
- [ ] T041 [US4] Add wiremock test: HTTP 401 → auth error (not retryable) in `adapters/tests/azure.rs`
- [ ] T042 [US4] Add wiremock test: HTTP 404 → non-retryable error in `adapters/tests/azure.rs`
- [ ] T043 [US4] Add wiremock test: HTTP 500 → network error (retryable) in `adapters/tests/azure.rs`
- [ ] T044 [US4] Add wiremock test: SSE stream with `finish_reason: "content_filter"` → ContentFiltered error event in `adapters/tests/azure.rs`
- [ ] T045 [US4] Add wiremock test: HTTP error body with `ContentFilterBlocked` code → ContentFiltered error event in `adapters/tests/azure.rs`
- [ ] T046 [US4] Add wiremock test: Entra ID token endpoint failure → network error in `adapters/tests/azure.rs`
- [ ] T047 [US4] Run `cargo test -p swink-agent-adapters --features azure` to verify all US4 tests pass

**Checkpoint**: All error scenarios correctly classified. Content filter violations surface as distinct error type.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Live tests, documentation, and final validation.

- [ ] T048 [P] Create live test file `adapters/tests/azure_live.rs` with `#[ignore]` tests for text streaming against real Azure deployment
- [ ] T049 [P] Add live test for tool call streaming in `adapters/tests/azure_live.rs`
- [ ] T050 [P] Add live test for error handling (invalid API key) in `adapters/tests/azure_live.rs`
- [ ] T051 Update `adapters/CLAUDE.md` — change Azure status from "Stub" to "Implemented", add auth/protocol notes to Lessons Learned
- [ ] T052 Run `cargo test --workspace` to verify no regressions across entire workspace
- [ ] T053 Run `cargo clippy --workspace -- -D warnings` to verify zero warnings
- [ ] T054 Run `cargo test -p swink-agent-adapters --features azure` as final validation of all Azure adapter tests

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — verification only
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational — MVP
- **US2 (Phase 4)**: Depends on US1 (needs AzureStreamFn struct changes)
- **US3 (Phase 5)**: Depends on US1 (needs AzureAuth enum and constructor)
- **US4 (Phase 6)**: Depends on Foundational (ContentFiltered) + US1 (stream pipeline)
- **Polish (Phase 7)**: Depends on all user stories

### User Story Dependencies

- **US1 (P1)**: After Foundational — defines core types (AzureAuth, constructor)
- **US2 (P1)**: After US1 — adds tool call tests (no code changes, inherits openai_compat)
- **US3 (P2)**: After US1 — adds Entra ID auth on top of US1 types
- **US4 (P2)**: After Foundational + US1 — adds content filter detection to stream pipeline

### Within Each User Story

- Type definitions before implementations
- Implementations before tests that exercise them
- Wiremock tests validate each acceptance scenario

### Parallel Opportunities

- T001 and T002 can run in parallel (Phase 1)
- T003, T004, T005, T006, T007 can run in parallel (different files/sections in Phase 2)
- T048, T049, T050 can run in parallel (independent live tests in Phase 7)
- US2 and US3 can start in parallel after US1 completes
- US4 can start after US1 completes (only needs Foundational + stream pipeline)

---

## Parallel Example: Phase 2 (Foundational)

```
# These modify different files and can run in parallel:
Task: "Add ContentFiltered to StreamErrorKind in src/stream.rs"       (T003)
Task: "Add ContentFiltered to AgentError in src/error.rs"             (T006)

# These depend on the above:
Task: "Add error_content_filtered constructor in src/stream.rs"       (T004, after T003)
Task: "Map ContentFiltered in loop error handling in src/loop_.rs"    (T008, after T003+T006)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup verification
2. Complete Phase 2: Foundational (ContentFiltered in core)
3. Complete Phase 3: User Story 1 (text streaming + API key auth)
4. **STOP and VALIDATE**: Test text streaming independently
5. Azure adapter is usable with API key auth

### Incremental Delivery

1. Setup + Foundational → Core has ContentFiltered support
2. US1 → Text streaming with API key auth (MVP!)
3. US2 → Tool call streaming verified (no code changes, just tests)
4. US3 → Azure AD/Entra ID auth added
5. US4 → Content filter detection and error classification
6. Each story adds value without breaking previous stories

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Existing `azure.rs` stub is being extended, not replaced
- Core crate changes (Phase 2) are small and surgical — 3 files touched
- The `openai_compat` module handles SSE parsing and tool calls — Azure adapter only customizes URL, auth, and content filter detection
- No new external dependencies needed
