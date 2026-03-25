# Tasks: Adapter: Google Gemini

**Input**: Design documents from `/specs/015-adapter-gemini/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Tests**: Included (constitution mandates TDD).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Define Gemini-specific types and public struct

- [x] T001 Define serializable request types (GeminiRequest, GeminiContent, GeminiPart, GeminiGenerationConfig, GeminiThinkingConfig, GeminiToolConfig, GeminiFunctionCallingConfig, GeminiTool, GeminiFunctionDeclaration, GeminiInlineData, GeminiFileData, GeminiFunctionCall, GeminiFunctionResponse) in adapters/src/google.rs
- [x] T002 Define deserializable response types (GeminiChunk, GeminiCandidate, GeminiResponseContent, GeminiResponsePart, GeminiUsageMetadata) in adapters/src/google.rs
- [x] T003 Define GeminiStreamState and GeminiToolCallState structs for stream state tracking in adapters/src/google.rs
- [x] T004 Implement GeminiStreamFn struct with new() constructor, api_version_path(), Debug (redacted key), and compile-time Send+Sync assertion in adapters/src/google.rs
- [x] T005 Add `mod google;` and `pub use google::GeminiStreamFn;` re-export in adapters/src/lib.rs

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Message conversion and request building — MUST complete before streaming stories

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T006 [US3] Implement convert_messages() to convert Vec<AgentMessage> to Vec<GeminiContent> handling User, Assistant, and ToolResult variants in adapters/src/google.rs
- [x] T007 [P] [US3] Implement user_parts() to convert user ContentBlocks to GeminiParts (text, images via inlineData/fileData) in adapters/src/google.rs
- [x] T008 [P] [US3] Implement assistant_parts() to convert assistant ContentBlocks to GeminiParts (text, thinking with thought flag and signature, function calls) and track tool_names_by_id in adapters/src/google.rs
- [x] T009 [P] [US3] Implement tool_result_part() to convert ToolResultMessage to GeminiFunctionResponse part using tool_names_by_id lookup in adapters/src/google.rs
- [x] T010 [US3] Implement build_tools() to convert Vec<Arc<dyn AgentTool>> to Vec<GeminiTool> using extract_tool_schemas() in adapters/src/google.rs
- [x] T011 [US3] Implement convert_request() to build GeminiRequest from AgentContext and StreamOptions (system_instruction, contents, tools, tool_config, generation_config with thinking detection) in adapters/src/google.rs
- [x] T012 [US3] Implement send_request() to POST to /{version}/models/{model}:streamGenerateContent?alt=sse with x-goog-api-key header and per-request api_key override in adapters/src/google.rs
- [x] T013 [US3] Write wiremock test for message conversion verifying system prompt, user text, assistant text, tool calls, tool results, and images are correctly serialized in adapters/tests/google.rs

**Checkpoint**: Foundation ready — streaming story implementation can now begin

---

## Phase 3: User Story 1 — Stream Text Responses (Priority: P1) 🎯 MVP

**Goal**: Stream text content incrementally from Gemini via SSE

**Independent Test**: Send a simple prompt to wiremock Gemini endpoint, verify text deltas arrive incrementally and Done event has correct usage

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T014 [US1] Write wiremock test google_text_stream verifying Start, TextStart, TextDelta("hello"), TextEnd, Done event sequence with usage (input=10, output=5, total=15) in adapters/tests/google.rs
- [x] T015 [P] [US1] Write wiremock test google_thinking_then_text_stream verifying ThinkingStart, ThinkingDelta, ThinkingEnd (with signature), then TextStart sequence ordering in adapters/tests/google.rs

### Implementation for User Story 1

- [x] T016 [US1] Implement StreamFn::stream() for GeminiStreamFn delegating to gemini_stream() in adapters/src/google.rs
- [x] T017 [US1] Implement gemini_stream() using stream::once + flatten pattern: call send_request, check HTTP status via error_event_from_status from shared classify module, delegate to parse_sse_stream in adapters/src/google.rs
- [x] T018 [US1] Implement parse_sse_stream() using stream::unfold over sse_data_lines: emit Start on first iteration, parse GeminiChunk JSON, call process_chunk, emit Done on stream end or [DONE] sentinel in adapters/src/google.rs
- [x] T019 [US1] Implement process_chunk() to extract first candidate, update usage from usage_metadata, and dispatch content parts in adapters/src/google.rs
- [x] T020 [US1] Implement text block handling in process_chunk: TextStart on first text part, TextDelta for each non-empty text, track text_content_index in GeminiStreamState in adapters/src/google.rs
- [x] T021 [US1] Implement thinking block handling in process_chunk: detect thought=true flag, emit ThinkingStart/ThinkingDelta/ThinkingEnd, buffer thoughtSignature, close text before thinking and vice versa in adapters/src/google.rs
- [x] T022 [US1] Implement close_text_block() and close_thinking_block() helpers for clean block transitions in adapters/src/google.rs
- [x] T023 [US1] Implement map_finish_reason() mapping STOP, MAX_TOKENS, and tool-call detection to StopReason variants in adapters/src/google.rs

**Checkpoint**: Text streaming works end-to-end with thinking support

---

## Phase 4: User Story 2 — Stream Tool Call Responses (Priority: P1)

**Goal**: Stream function call events with correct names, IDs, and JSON arguments

**Independent Test**: Send a prompt with tool definitions to wiremock endpoint, verify ToolCallStart/Delta/End events with correct function name and parseable arguments

### Tests for User Story 2

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T024 [P] [US2] Write wiremock test google_tool_call_stream verifying ToolCallStart (id="call_1", name="get_weather"), ToolCallDelta (args={"city":"Paris"}), ToolCallEnd, and Done with StopReason::ToolUse in adapters/tests/google.rs
- [x] T025 [P] [US2] Write wiremock test google_multiple_tool_calls verifying two function calls in a single response each emit separate indexed ToolCallStart/Delta/End blocks in adapters/tests/google.rs

### Implementation for User Story 2

- [x] T026 [US2] Implement process_function_call() to handle function call parts: create GeminiToolCallState entry, emit ToolCallStart with id/name, compute argument deltas by comparing serialized args to accumulated state in adapters/src/google.rs
- [x] T027 [US2] Integrate function call handling into process_chunk: detect functionCall parts, close text/thinking blocks before processing, set saw_tool_call flag in adapters/src/google.rs
- [x] T028 [US2] Handle generated tool call IDs when provider id is absent (format: "gemini-tool-{part_index}") in adapters/src/google.rs

**Checkpoint**: Tool call streaming works with correct argument deltas and stop reason

---

## Phase 5: User Story 4 — Handle Errors from Google Gemini (Priority: P2)

**Goal**: Classify HTTP errors and surface safety filter blocks as errors

**Independent Test**: Simulate error responses (401, 403, 429, 500, timeout) via wiremock, verify each maps to correct error type; simulate SAFETY finish reason, verify error event

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T029 [P] [US4] Write wiremock test google_http_429_maps_to_throttled verifying error_throttled event for HTTP 429 response in adapters/tests/google.rs
- [x] T030 [P] [US4] Write wiremock test google_http_401_maps_to_auth verifying error_auth event for HTTP 401 response in adapters/tests/google.rs
- [x] T031 [P] [US4] Write wiremock test google_http_403_maps_to_auth verifying error_auth event for HTTP 403 response in adapters/tests/google.rs
- [x] T032 [P] [US4] Write wiremock test google_http_500_maps_to_network verifying error_network event for HTTP 500 response in adapters/tests/google.rs
- [x] T033 [P] [US4] Write wiremock test google_connection_error_maps_to_network verifying error_network event when the server is unreachable in adapters/tests/google.rs
- [x] T034 [US4] Write wiremock test google_safety_finish_reason_emits_error verifying AssistantMessageEvent::error() when finish_reason is "SAFETY" in adapters/tests/google.rs

### Implementation for User Story 4

- [x] T035 [US4] Handle connection errors in send_request() mapping reqwest errors to error_network events in adapters/src/google.rs
- [x] T036 [US4] Handle JSON parse errors in parse_sse_stream() emitting error event with descriptive message in adapters/src/google.rs
- [x] T037 [US4] Implement safety filter handling: detect finish_reason "SAFETY" in process_chunk and emit AssistantMessageEvent::error() with descriptive message instead of StopReason::Stop in adapters/src/google.rs

**Checkpoint**: All error codes and safety blocks map to correct agent error types

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Stream finalization, cancellation support, and live integration tests

- [x] T038 Implement StreamFinalize for GeminiStreamState: drain_open_blocks() closing text, thinking, and tool call blocks in content_index order in adapters/src/google.rs
- [x] T039 Integrate cancellation_token checking in parse_sse_stream loop for clean cancellation support in adapters/src/google.rs
- [x] T040 [P] Write live integration test streaming_text_response using GEMINI_API_KEY with 30s timeout validating Start/TextStart/TextDelta/TextEnd/Done sequence in adapters/tests/google_live.rs
- [x] T041 [P] Write live integration test streaming_tool_call using GEMINI_API_KEY with tool definitions validating ToolCallStart/ToolCallDelta/ToolCallEnd sequence in adapters/tests/google_live.rs
- [x] T042 Run cargo test -p swink-agent-adapters and cargo clippy --workspace -- -D warnings to verify all tests pass and zero warnings
- [x] T043 Run cargo test --workspace to verify no regressions across the workspace
- [x] T044 Run quickstart.md validation: verify build, test, and usage example commands work

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Stories (Phases 3–5)**: All depend on Foundational phase completion
  - US1 and US2 can proceed in parallel (different event types, shared chunk parsing)
  - US4 can proceed in parallel with US1/US2 (error path is independent of happy path)
- **Polish (Phase 6)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational (Phase 2) — No dependencies on other stories
- **User Story 2 (P1)**: Can start after Foundational (Phase 2) — Shares process_chunk with US1 but targets different code paths (functionCall parts)
- **User Story 3 (P2)**: Implemented in Foundational phase — it IS the foundation that other stories depend on
- **User Story 4 (P2)**: Can start after Foundational (Phase 2) — Error handling is orthogonal to happy-path streaming

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Types before functions
- Helper functions before main stream logic
- Core implementation before integration
- Story complete before moving to next priority

### Parallel Opportunities

- T007, T008, T009 can run in parallel (different conversion functions, no shared state)
- T014, T015 can run in parallel (different test functions)
- T024, T025 can run in parallel (different tool call test functions)
- T029, T030, T031, T032, T033 can run in parallel (different error code tests)
- T040, T041 can run in parallel (different live test scenarios)
- Once Foundational completes: US1, US2, US4 phases can start in parallel

---

## Parallel Example: User Story 1

```bash
# Launch tests for User Story 1 together:
Task: "Write wiremock test google_text_stream in adapters/tests/google.rs"
Task: "Write wiremock test google_thinking_then_text_stream in adapters/tests/google.rs"

# After tests, launch parallel implementation:
Task: "Implement text block handling in process_chunk in adapters/src/google.rs"
Task: "Implement thinking block handling in process_chunk in adapters/src/google.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (types and struct)
2. Complete Phase 2: Foundational (message conversion)
3. Complete Phase 3: User Story 1 (text streaming)
4. **STOP and VALIDATE**: Test text streaming independently
5. Demo with simple text prompt

### Incremental Delivery

1. Complete Setup + Foundational → Foundation ready
2. Add User Story 1 (text streaming) → Test independently → MVP!
3. Add User Story 2 (tool calls) → Test independently → Agentic capability
4. Add User Story 4 (error handling + safety) → Test independently → Production-ready
5. Polish → StreamFinalize, live tests → Ship

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together
2. Once Foundational is done:
   - Developer A: User Story 1 (text streaming)
   - Developer B: User Story 2 (tool calls)
   - Developer C: User Story 4 (error handling)
3. Stories complete and integrate independently

---

## Notes

- [P] tasks = different files or functions, no dependencies
- [Story] label maps task to specific user story for traceability
- US3 (message conversion) is implemented as the Foundational phase since US1 and US2 depend on it
- All code lives in a single file (adapters/src/google.rs) — parallel tasks target different functions, not different files
- Wiremock tests in adapters/tests/google.rs; live tests in adapters/tests/google_live.rs
- Live tests require GEMINI_API_KEY environment variable and are marked #[ignore]
