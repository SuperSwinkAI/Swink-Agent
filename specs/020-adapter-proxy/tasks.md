# Tasks: Adapter: Proxy

**Input**: Design documents from `/specs/020-adapter-proxy/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md, quickstart.md

**Tests**: Tests are included per the spec (constitution check cites 12 unit tests).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Module scaffolding and dependency wiring

- [ ] T001 Create `adapters/src/proxy.rs` module file with `#![forbid(unsafe_code)]` doc comment and module-level documentation
- [ ] T002 Register `mod proxy;` in `adapters/src/lib.rs` and add `pub use proxy::ProxyStreamFn` re-export
- [ ] T003 Verify `eventsource-stream` is listed in workspace `Cargo.toml` dependencies and added to `adapters/Cargo.toml`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Request and SSE event types that all user stories depend on

**CRITICAL**: No user story work can begin until this phase is complete

- [ ] T004 [P] Define `ProxyRequest<'a>` struct with `model`, `system`, `messages`, `options` fields and `#[derive(Serialize)]` in `adapters/src/proxy.rs`
- [ ] T005 [P] Define `ProxyRequestOptions<'a>` struct with `temperature`, `max_tokens`, `session_id` (all `Option`, `skip_serializing_if`) and `#[derive(Serialize)]` in `adapters/src/proxy.rs`
- [ ] T006 Define `SseEventData` enum with all 12 variants (`Start`, `TextStart`, `TextDelta`, `TextEnd`, `ThinkingStart`, `ThinkingDelta`, `ThinkingEnd`, `ToolCallStart`, `ToolCallDelta`, `ToolCallEnd`, `Done`, `Error`) using `#[serde(tag = "type", rename_all = "snake_case")]` in `adapters/src/proxy.rs`
- [ ] T007 Implement `convert_sse_event()` function mapping each `SseEventData` variant 1:1 to `AssistantMessageEvent` in `adapters/src/proxy.rs`
- [ ] T008 Implement `parse_sse_event_data()` function that deserializes JSON string to `SseEventData` and calls `convert_sse_event`, with malformed JSON yielding `Error` event in `adapters/src/proxy.rs`
- [ ] T009 Implement `is_terminal_event()` const fn that returns true for `Done` and `Error` variants in `adapters/src/proxy.rs`

**Checkpoint**: Foundation ready — all types and parsing functions in place

---

## Phase 3: User Story 1 — Stream Text Responses Through a Proxy (Priority: P1)

**Goal**: Developer configures proxy with URL and bearer token, sends conversation, receives incremental text deltas via SSE.

**Independent Test**: Send a simple prompt through a proxy endpoint and verify text deltas arrive incrementally and the final assembled message is coherent.

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T010 [P] [US1] Unit test `parse_start_event` — verify `{"type":"start"}` parses to `AssistantMessageEvent::Start` in `adapters/src/proxy.rs`
- [ ] T011 [P] [US1] Unit test `parse_text_delta_event` — verify `{"type":"text_delta","content_index":0,"delta":"hello"}` parses correctly in `adapters/src/proxy.rs`
- [ ] T012 [P] [US1] Unit test `parse_done_event` — verify `{"type":"done",...}` parses with correct `stop_reason`, `usage`, and `cost` in `adapters/src/proxy.rs`
- [ ] T013 [P] [US1] Unit test `is_terminal_detects_done_and_error` — verify `Done` and `Error` are terminal, `Start` is not in `adapters/src/proxy.rs`

### Implementation for User Story 1

- [ ] T014 [US1] Define `ProxyStreamFn` struct with `base_url: String`, `bearer_token: String`, `client: reqwest::Client` in `adapters/src/proxy.rs`
- [ ] T015 [US1] Implement `ProxyStreamFn::new(base_url, bearer_token)` constructor with `#[must_use]` and `Client::new()` in `adapters/src/proxy.rs`
- [ ] T016 [US1] Implement `send_request()` async fn — build `ProxyRequest` from `ModelSpec`/`AgentContext`/`StreamOptions`, filter out `CustomMessage`, POST to `{base_url}/v1/stream` with bearer auth in `adapters/src/proxy.rs`
- [ ] T017 [US1] Implement `parse_sse_stream()` — use `response.bytes_stream().eventsource()`, `stream::unfold` with cancellation via `tokio::select!`, emit events until terminal in `adapters/src/proxy.rs`
- [ ] T018 [US1] Implement `proxy_stream()` — orchestrate `send_request` then `classify_response_status` then `parse_sse_stream`, flatten into single stream in `adapters/src/proxy.rs`
- [ ] T019 [US1] Implement `StreamFn` trait for `ProxyStreamFn` — delegate `stream()` to `proxy_stream()` in `adapters/src/proxy.rs`
- [ ] T020 [US1] Add compile-time `Send + Sync` assertion for `ProxyStreamFn` via `const` block in `adapters/src/proxy.rs`

**Checkpoint**: US1 complete — text streaming through proxy works end-to-end

---

## Phase 4: User Story 2 — Handle All Delta Types (Priority: P1)

**Goal**: The proxy adapter handles not just text deltas but also thinking deltas and tool call deltas, all mapped 1:1 from typed SSE events. (Note: per research.md Decision 2, the proxy protocol uses discrete typed events — no partial_message diffing needed.)

**Independent Test**: Feed SSE events for thinking and tool call variants and verify correct `AssistantMessageEvent` mapping.

### Tests for User Story 2

- [ ] T021 [P] [US2] Unit test `parse_thinking_end_event` — verify `{"type":"thinking_end","content_index":1,"signature":"sig123"}` parses correctly in `adapters/src/proxy.rs`
- [ ] T022 [P] [US2] Unit test `parse_tool_call_start_event` — verify `{"type":"tool_call_start","content_index":2,"id":"tc_1","name":"read_file"}` parses correctly in `adapters/src/proxy.rs`
- [ ] T023 [P] [US2] Unit test `parse_thinking_delta_event` — verify `{"type":"thinking_delta","content_index":1,"delta":"reasoning"}` parses to correct `ThinkingDelta` variant in `adapters/src/proxy.rs`
- [ ] T024 [P] [US2] Unit test `parse_tool_call_delta_event` — verify `{"type":"tool_call_delta","content_index":2,"delta":"{\"path\":"}` parses to correct `ToolCallDelta` variant in `adapters/src/proxy.rs`

**Checkpoint**: US2 complete — all SSE event types (text, thinking, tool call) handled

---

## Phase 5: User Story 3 — Authenticate with Bearer Tokens (Priority: P2)

**Goal**: Bearer token included in every request; invalid tokens surface as authentication errors; token redacted in Debug output.

**Independent Test**: Send request with valid token and verify success; send with invalid token and verify auth error.

### Tests for User Story 3

- [ ] T025 [P] [US3] Unit test `proxy_stream_fn_debug_redacts_token` — verify Debug output contains `[redacted]` and not the actual token in `adapters/src/proxy.rs`
- [ ] T026 [P] [US3] Unit test `auth_error_contains_status` — verify `error_auth("authentication failure (401)")` contains expected text in `adapters/src/proxy.rs`

### Implementation for User Story 3

- [ ] T027 [US3] Implement `Debug` for `ProxyStreamFn` — redact `bearer_token` as `"[redacted]"` using `finish_non_exhaustive()` in `adapters/src/proxy.rs`
- [ ] T028 [US3] Ensure `send_request()` uses `StreamOptions.api_key` as override when `Some`, falling back to stored `bearer_token` in `adapters/src/proxy.rs`

**Checkpoint**: US3 complete — authentication works with token redaction

---

## Phase 6: User Story 4 — Classify Proxy-Specific Errors (Priority: P2)

**Goal**: Connection failures, auth errors, rate-limit errors, and malformed responses each classified correctly for the retry strategy.

**Independent Test**: Simulate various error conditions and verify each maps to the correct error type.

### Tests for User Story 4

- [ ] T029 [P] [US4] Unit test `malformed_json_yields_error_event` — verify invalid JSON produces `Error` with "malformed SSE event JSON" message in `adapters/src/proxy.rs`
- [ ] T030 [P] [US4] Unit test `network_error_uses_canonical_constructor` — verify `error_network` contains "network error" in `adapters/src/proxy.rs`
- [ ] T031 [P] [US4] Unit test `rate_limit_error_contains_429` — verify `error_throttled("rate limit (429)")` contains expected text in `adapters/src/proxy.rs`
- [ ] T032 [P] [US4] Unit test `aborted_has_correct_stop_reason` — verify cancellation event has `StopReason::Aborted` in `adapters/src/proxy.rs`

### Implementation for User Story 4

- [ ] T033 [US4] Implement `classify_response_status()` — delegate to `classify_http_status` from `crate::classify`, map `HttpErrorKind::Auth` to `error_auth`, `Throttled` to `error_throttled`, `Network`/unknown to `error_network` in `adapters/src/proxy.rs`
- [ ] T034 [US4] Ensure `parse_sse_stream()` handles stream errors (`Some(Err(e))`) as `error_network` and unexpected stream end (`None`) as `error_network("SSE stream ended unexpectedly")` in `adapters/src/proxy.rs`
- [ ] T035 [US4] Ensure cancellation path in `parse_sse_stream()` emits `Error` with `StopReason::Aborted` via `tokio::select!` in `adapters/src/proxy.rs`

**Checkpoint**: US4 complete — all four error categories classified correctly

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Final validation across all stories

- [ ] T036 Run `cargo build -p swink-agent-adapters` and verify clean compilation
- [ ] T037 Run `cargo test -p swink-agent-adapters proxy` and verify all proxy tests pass
- [ ] T038 Run `cargo clippy --workspace -- -D warnings` and fix any warnings
- [ ] T039 Run `cargo test --workspace` and verify no regressions
- [ ] T040 Validate quickstart.md examples match the implemented API in `specs/020-adapter-proxy/quickstart.md`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational (Phase 2) — core streaming
- **US2 (Phase 4)**: Depends on Foundational (Phase 2) — can run in parallel with US1
- **US3 (Phase 5)**: Depends on Foundational (Phase 2) — can run in parallel with US1/US2
- **US4 (Phase 6)**: Depends on Foundational (Phase 2) — can run in parallel with US1/US2/US3
- **Polish (Phase 7)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational — no dependencies on other stories
- **User Story 2 (P1)**: Can start after Foundational — independent of US1 (shares `SseEventData` from Foundational)
- **User Story 3 (P2)**: Can start after Foundational — independent (auth is in `send_request`)
- **User Story 4 (P2)**: Can start after Foundational — independent (error classification is separate functions)

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Types/structs before functions
- Functions before trait impl
- Core implementation before integration

### Parallel Opportunities

- T004 and T005 can run in parallel (different structs)
- T010-T013 can all run in parallel (independent test functions)
- T021-T024 can all run in parallel (independent test functions)
- T025-T026 can all run in parallel (independent test functions)
- T029-T032 can all run in parallel (independent test functions)
- US1, US2, US3, US4 phases can all start in parallel after Foundational phase

---

## Parallel Example: User Story 1

```bash
# Launch all tests for US1 together:
Task T010: "Unit test parse_start_event in adapters/src/proxy.rs"
Task T011: "Unit test parse_text_delta_event in adapters/src/proxy.rs"
Task T012: "Unit test parse_done_event in adapters/src/proxy.rs"
Task T013: "Unit test is_terminal_detects_done_and_error in adapters/src/proxy.rs"
```

## Parallel Example: User Story 4

```bash
# Launch all tests for US4 together:
Task T029: "Unit test malformed_json_yields_error_event in adapters/src/proxy.rs"
Task T030: "Unit test network_error_uses_canonical_constructor in adapters/src/proxy.rs"
Task T031: "Unit test rate_limit_error_contains_429 in adapters/src/proxy.rs"
Task T032: "Unit test aborted_has_correct_stop_reason in adapters/src/proxy.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (module scaffolding)
2. Complete Phase 2: Foundational (types and parsing)
3. Complete Phase 3: User Story 1 (text streaming)
4. **STOP and VALIDATE**: Test US1 independently with `cargo test -p swink-agent-adapters proxy`
5. Verify text deltas stream correctly

### Incremental Delivery

1. Complete Setup + Foundational -> Foundation ready
2. Add US1 (text streaming) -> Test independently (MVP!)
3. Add US2 (thinking + tool call events) -> Test independently
4. Add US3 (bearer auth + redaction) -> Test independently
5. Add US4 (error classification) -> Test independently
6. Each story adds value without breaking previous stories

---

## Notes

- [P] tasks = different files/functions, no dependencies
- [Story] label maps task to specific user story for traceability
- Per research.md Decision 2: No delta reconstruction needed — proxy uses typed SSE events
- Per research.md Decision 4: Reuse `classify_http_status` from `crate::classify` (spec 011)
- All implementation lives in single file `adapters/src/proxy.rs` per plan.md
- Total tasks: 40
