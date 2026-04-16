# Tasks: Adapter — Mistral

**Input**: Design documents from `/specs/018-adapter-mistral/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Full test parity with OpenAI adapter required by spec (FR-008).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Refactor MistralStreamFn from OpenAiStreamFn wrapper to AdapterBase holder and set up foundational types

- [x] T001 Refactor `MistralStreamFn` to hold `AdapterBase` instead of `OpenAiStreamFn` in `adapters/src/mistral.rs`
- [x] T002 Implement `MistralStreamFn::new(base_url, api_key)` constructor using `AdapterBase::new()` in `adapters/src/mistral.rs`
- [x] T003 Implement `Debug` for `MistralStreamFn` (redact api_key) in `adapters/src/mistral.rs`
- [x] T004 [P] Implement `MistralIdMap` struct with `new()`, `insert(harness_id) -> mistral_id`, and `to_harness(mistral_id) -> harness_id` methods using `rand` for 9-char `[a-zA-Z0-9]` ID generation in `adapters/src/mistral.rs`
- [x] T005 [P] Implement `send_request()` function that constructs POST to `{base_url}/v1/chat/completions` with `Authorization: Bearer` header, serializes `OaiChatRequest` body (using `max_tokens` not `max_completion_tokens`, omitting `stream_options`), and returns the response or error event via `classify::error_event_from_status` in `adapters/src/mistral.rs`

**Checkpoint**: Foundation ready — `MistralStreamFn` compiles with `AdapterBase`, ID mapping, and request sending

---

## Phase 2: Foundational (Message Conversion & Response Normalization)

**Purpose**: Request and response normalization that ALL user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T006 Implement message conversion function that uses `OaiConverter` but remaps tool call IDs in assistant replay messages from harness format to Mistral 9-char format via `MistralIdMap`, and inserts synthetic `{"role":"assistant","content":""}` between consecutive tool-result and user messages in `adapters/src/mistral.rs`
- [x] T007 Implement response normalization as a stream combinator (map over `AssistantMessageEvent`) that remaps tool call IDs from Mistral 9-char format back to harness format via `MistralIdMap`, maps `finish_reason: "model_length"` to `StopReason::MaxTokens`, and converts `finish_reason: "error"` to an error event in `adapters/src/mistral.rs`
- [x] T008 Wire `StreamFn::stream()` implementation: build `MistralIdMap` from context, convert messages (T006), construct normalized `OaiChatRequest`, call `send_request()` (T005), pipe response through `parse_oai_sse_stream`, apply response normalizer (T007), return event stream in `adapters/src/mistral.rs`

**Checkpoint**: `MistralStreamFn` implements `StreamFn` with full request/response normalization — ready for story-level testing

---

## Phase 3: User Story 1 — Stream Text Responses (Priority: P1) 🎯 MVP

**Goal**: Text content streams incrementally from Mistral via SSE with correct event sequence

**Independent Test**: Send a simple prompt to mock Mistral endpoint, verify TextStart/TextDelta/TextEnd/Done events arrive in order

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T009 [P] [US1] Test text streaming happy path: mock server returns SSE chunks with text deltas, verify Start → TextStart → TextDelta("ok") → TextEnd → Done event sequence in `adapters/tests/mistral.rs`
- [x] T010 [P] [US1] Test usage tracking: mock server returns usage in final chunk (no `stream_options` sent), verify `Usage` fields (prompt_tokens, completion_tokens) extracted correctly in `adapters/tests/mistral.rs`
- [x] T011 [P] [US1] Test request body: mock server captures request, verify `max_tokens` field present (not `max_completion_tokens`), `stream_options` absent, `stream: true` present, `Authorization: Bearer` header correct in `adapters/tests/mistral.rs`
- [x] T012 [P] [US1] Test `model_length` finish reason: mock server returns `finish_reason: "model_length"`, verify it maps to `StopReason::MaxTokens` in Done event in `adapters/tests/mistral.rs`
- [x] T013 [P] [US1] Test stream cancellation: cancel `CancellationToken` mid-stream, verify open text block is closed with TextEnd and error event emitted in `adapters/tests/mistral.rs`
- [x] T014 [P] [US1] Test multi-chunk text assembly: mock server returns 5+ text delta chunks, verify all deltas arrive as separate TextDelta events and final message is coherent in `adapters/tests/mistral.rs`

### Implementation for User Story 1

- [x] T015 [US1] Verify text streaming works end-to-end: run all US1 tests, ensure they pass with the StreamFn implementation from Phase 2 in `adapters/src/mistral.rs`

**Checkpoint**: Text streaming fully functional and tested — MVP complete

---

## Phase 4: User Story 2 — Stream Tool Call Responses (Priority: P1)

**Goal**: Tool calls stream with correct IDs, argument accumulation, and multi-tool support

**Independent Test**: Send prompt with tool definitions to mock Mistral endpoint, verify ToolCallStart/ToolCallDelta/ToolCallEnd events with valid JSON arguments and harness-format IDs

### Tests for User Story 2

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T016 [P] [US2] Test single tool call streaming: mock server returns tool call chunks, verify ToolCallStart (name + harness-format ID) → ToolCallDelta (argument fragments) → ToolCallEnd (valid JSON) event sequence in `adapters/tests/mistral.rs`
- [x] T017 [P] [US2] Test multi-tool call streaming: mock server returns 2+ tool calls with different indices, verify each tool call emitted as separate indexed block with its own harness-format ID in `adapters/tests/mistral.rs`
- [x] T018 [P] [US2] Test tool call ID format: verify outbound request contains 9-char `[a-zA-Z0-9]` IDs for tool result messages, and inbound events contain harness-format `call_*` IDs in `adapters/tests/mistral.rs`
- [x] T019 [P] [US2] Test full tool call in single chunk: mock server returns complete tool call (not incremental), verify adapter handles it correctly (name + full arguments in one chunk) in `adapters/tests/mistral.rs`
- [x] T020 [P] [US2] Test tool definitions in request: mock server captures request, verify `tools` array contains correct function schemas and `tool_choice: "auto"` when tools provided in `adapters/tests/mistral.rs`

### Implementation for User Story 2

- [x] T021 [US2] Verify tool call streaming works end-to-end: run all US2 tests, ensure they pass — tool call events emitted with harness-format IDs, multi-tool supported in `adapters/src/mistral.rs`

**Checkpoint**: Tool call streaming fully functional — both P1 stories complete

---

## Phase 5: User Story 3 — Mistral-Specific Endpoint (Priority: P2)

**Goal**: Adapter connects to Mistral endpoint with correct URL, auth, and handles format differences transparently

**Independent Test**: Verify correct URL construction, Bearer auth header, and message ordering constraint handling

### Tests for User Story 3

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T022 [P] [US3] Test endpoint URL construction: verify request targets `{base_url}/v1/chat/completions` with trailing-slash-safe base URL handling in `adapters/tests/mistral.rs`
- [x] T023 [P] [US3] Test Bearer auth header: verify `Authorization: Bearer {api_key}` header present on all requests in `adapters/tests/mistral.rs`
- [x] T024 [P] [US3] Test message ordering normalization: provide context with `[..., tool_result, user_message]` sequence, verify request body inserts synthetic assistant message between them in `adapters/tests/mistral.rs`
- [x] T025 [P] [US3] Test `finish_reason: "error"` handling: mock server returns `finish_reason: "error"`, verify adapter converts to error event in `adapters/tests/mistral.rs`

### Implementation for User Story 3

- [x] T026 [US3] Verify endpoint-specific behavior works end-to-end: run all US3 tests, ensure they pass — correct URL, auth, message ordering, and error finish_reason handling in `adapters/src/mistral.rs`

**Checkpoint**: Mistral-specific endpoint handling complete and tested

---

## Phase 6: User Story 4 — Error Handling (Priority: P2)

**Goal**: HTTP errors from Mistral classified correctly for retry strategy

**Independent Test**: Simulate 401/429/500/timeout responses, verify each maps to correct error type

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T027 [P] [US4] Test HTTP 429 → rate-limit error (retryable): mock server returns 429, verify `AssistantMessageEvent::error_throttled()` emitted in `adapters/tests/mistral.rs`
- [x] T028 [P] [US4] Test HTTP 401 → auth error (not retryable): mock server returns 401, verify `AssistantMessageEvent::error_auth()` emitted in `adapters/tests/mistral.rs`
- [x] T029 [P] [US4] Test HTTP 500 → network error (retryable): mock server returns 500, verify `AssistantMessageEvent::error_network()` emitted in `adapters/tests/mistral.rs`
- [x] T030 [P] [US4] Test HTTP 422 → validation error: mock server returns 422 with Mistral error body, verify error event emitted with descriptive message in `adapters/tests/mistral.rs`

### Implementation for User Story 4

- [x] T031 [US4] Verify error handling works end-to-end: run all US4 tests, ensure they pass — all HTTP error codes map to correct event types in `adapters/src/mistral.rs`

**Checkpoint**: Error handling complete — all user stories functional

---

## Phase 7: Model Catalog & Presets

**Purpose**: Comprehensive Mistral model catalog and preset key expansion

- [x] T032 [P] Expand Mistral provider presets in `src/model_catalog.toml`: add `mistral_large` (mistral-large-latest, 256K, text/tools/images_in/streaming/structured_output), `ministral_3b` (ministral-3b-2512, 256K, text/images_in/streaming), `ministral_8b` (ministral-8b-2512, 256K, text/images_in/streaming), `ministral_14b` (ministral-14b-2512, 256K, text/images_in/streaming), `magistral_medium` (magistral-medium-2509, 40K, text/tools/streaming), `magistral_small` (magistral-small-2509, 40K, text/tools/streaming), `devstral` (devstral-2512, 256K, text/tools/streaming), `pixtral_large` (pixtral-large-2411, 128K, text/tools/images_in/streaming), `pixtral_12b` (pixtral-12b-2409, 128K, text/images_in/streaming) — update existing `mistral_medium`/`mistral_small`/`codestral` context windows to match current API docs
- [x] T033 [P] Add new preset keys in `adapters/src/remote_presets.rs`: `MISTRAL_LARGE`, `MINISTRAL_3B`, `MINISTRAL_8B`, `MINISTRAL_14B`, `MAGISTRAL_MEDIUM`, `MAGISTRAL_SMALL`, `DEVSTRAL`, `PIXTRAL_LARGE`, `PIXTRAL_12B` in the `mistral` module
- [x] T034 [P] Add test verifying all new Mistral preset keys resolve to catalog models in `adapters/src/remote_presets.rs` (extend existing `added_provider_presets_map_to_catalog_models` test)

**Checkpoint**: All 12 Mistral models available as presets

---

## Phase 8: Live Integration Test

**Purpose**: Validate against real Mistral API

- [x] T035 Create live integration test file `adapters/tests/mistral_live.rs` with `#[ignore]` attribute, requiring `MISTRAL_API_KEY` env var, 30s timeout
- [x] T036 [P] Implement live text streaming test: send simple prompt to `mistral-small-latest`, verify Start → TextDelta+ → Done event sequence in `adapters/tests/mistral_live.rs`
- [x] T037 [P] Implement live tool call round-trip test: send prompt with tool definition, verify tool call events with valid JSON arguments, then send tool result and verify follow-up response in `adapters/tests/mistral_live.rs`

**Checkpoint**: Live tests pass against real Mistral API

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Documentation, cleanup, and final validation

- [x] T038 [P] Update `adapters/AGENTS.md` lessons learned with Mistral-specific divergences (tool call ID format, `max_tokens`, no `stream_options`, message ordering constraint, `model_length` finish reason)
- [x] T039 [P] Verify `adapters/src/lib.rs` re-exports and feature gate are correct — `MistralStreamFn` available when `mistral` feature enabled, dead_code suppression includes `mistral` in the openai_compat gate
- [x] T040 Run `cargo test --workspace` to verify no regressions
- [x] T041 Run `cargo clippy --workspace -- -D warnings` to verify zero warnings
- [x] T042 Run quickstart.md validation — verify code examples compile conceptually against public API

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup (Phase 1) — BLOCKS all user stories
- **User Stories (Phases 3-6)**: All depend on Foundational (Phase 2) completion
  - US1 and US2 (both P1) can proceed in parallel after Phase 2
  - US3 and US4 (both P2) can proceed in parallel after Phase 2
  - All four stories are independent of each other
- **Catalog (Phase 7)**: Independent — can run in parallel with user stories
- **Live Tests (Phase 8)**: Depends on Phases 3-6 (all stories implemented)
- **Polish (Phase 9)**: Depends on all previous phases

### User Story Dependencies

- **US1 (Text Streaming)**: Depends on Phase 2 only — no cross-story deps
- **US2 (Tool Calls)**: Depends on Phase 2 only — no cross-story deps
- **US3 (Endpoint Config)**: Depends on Phase 2 only — no cross-story deps
- **US4 (Error Handling)**: Depends on Phase 2 only — no cross-story deps

### Within Each User Story

- Tests MUST be written and FAIL before implementation verification
- Each story test suite validates the shared Phase 2 implementation from its own angle

### Parallel Opportunities

- T004 and T005 can run in parallel (Phase 1)
- All test tasks within a story marked [P] can run in parallel
- T032, T033, T034 can all run in parallel (Phase 7)
- T036 and T037 can run in parallel (Phase 8)
- T038 and T039 can run in parallel (Phase 9)
- US1 and US2 can be worked in parallel (both P1)
- US3 and US4 can be worked in parallel (both P2)
- Phase 7 (Catalog) can run in parallel with Phases 3-6

---

## Parallel Example: User Story 2 (Tool Calls)

```bash
# Launch all tests for User Story 2 together:
Task: "T016 Test single tool call streaming in adapters/tests/mistral.rs"
Task: "T017 Test multi-tool call streaming in adapters/tests/mistral.rs"
Task: "T018 Test tool call ID format in adapters/tests/mistral.rs"
Task: "T019 Test full tool call in single chunk in adapters/tests/mistral.rs"
Task: "T020 Test tool definitions in request in adapters/tests/mistral.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (refactor to AdapterBase, ID map, send_request)
2. Complete Phase 2: Foundational (message conversion, response normalization, StreamFn wiring)
3. Complete Phase 3: User Story 1 (text streaming tests + verification)
4. **STOP and VALIDATE**: Text streaming works end-to-end
5. This alone is a usable Mistral adapter

### Incremental Delivery

1. Setup + Foundational → Core adapter ready
2. Add US1 (text) → Test independently → MVP!
3. Add US2 (tools) → Test independently → Agentic workflows enabled
4. Add US3 (endpoint) → Test independently → Mistral-specific handling proven
5. Add US4 (errors) → Test independently → Production-ready error handling
6. Add Catalog → All models available as presets
7. Add Live Tests → Validated against real API
8. Polish → Documentation and CI validation

---

## Notes

- [P] tasks = different files or independent code sections, no dependencies
- [Story] label maps task to specific user story for traceability
- The core implementation lives in Phase 2 — user story phases primarily add tests that validate different aspects of that implementation
- All test files use `wiremock` mock HTTP server (already a dev-dependency)
- Live tests require `MISTRAL_API_KEY` env var and are `#[ignore]` by default
- Tool call ID generation uses `rand` (already in workspace dependencies)
