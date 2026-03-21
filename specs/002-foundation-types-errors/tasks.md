# Tasks: Foundation Types & Errors

**Input**: Design documents from `/specs/002-foundation-types-errors/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Tests**: The spec requires test-driven development. Tests are included for each user story.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

**Context**: The workspace scaffold (001) already provides working implementations of most types in `src/types.rs` and `src/error.rs`. Tasks focus on aligning existing code with the 002 spec contracts, adding missing types/variants, and ensuring comprehensive test coverage per spec requirements. For scaffolded code, "verify" tasks confirm existing behavior satisfies spec contracts — tests are still written first per phase ordering (Constitution II), but the red-green-refactor cycle applies to new/changed code, not pre-existing scaffold.

**Scaffold Deviations**: The scaffold made several intentional design decisions that differ from the original spec/contracts. These are documented here as canonical (the code is authoritative since other modules already depend on it):
- **Usage/Cost field names**: Spec says `input_tokens`, `output_tokens`, etc. Code uses shorter `input`, `output`, etc. Code is canonical.
- **Timestamp type**: Spec/research says `SystemTime`. Code uses `u64` (epoch millis). Code is canonical — simpler serde, no custom module needed.
- **LlmMessage structure**: Contract shows inline enum fields. Code uses struct-wrapped variants (`User(UserMessage)`, etc.). Code is canonical — structs enable independent construction and reuse.
- **AssistantMessage field**: Contract says `model`. Code uses `model_id`. Code is canonical.
- **ThinkingBudgets value type**: Data model says `u32`. Code uses `u64`. Code is canonical — consistent with Usage counters.
- **NetworkError fields**: Contract shows no fields. Code has `source: Box<dyn Error + Send + Sync>`. Code is canonical — richer error chaining.
- **CustomMessage trait bounds**: Spec says `Send + Sync + Any`. Code also requires `Debug`. Code is canonical — Debug is essential for error reporting.
- **CustomMessage::type_name return**: Spec says `&str`. Code returns `Option<&str>` (supports non-serializable customs). Code is canonical.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Verify scaffold state and prepare for spec-aligned changes

- [ ] T001 Verify workspace builds cleanly and all existing tests pass by running `cargo test --workspace` and `cargo clippy --workspace -- -D warnings`
- [ ] T002 Create shared test helpers module at `tests/common/mod.rs` with helper functions for constructing test messages, content blocks, and usage/cost records used across integration test files

**Checkpoint**: Scaffold verified, test helpers ready

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Align existing type infrastructure with spec contracts before user story work begins

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [ ] T003 Add `DowncastError` struct to `src/error.rs` with fields `expected: &'static str` and `actual: String`, deriving `Debug` and implementing `thiserror::Error` with display message `"Downcast failed: expected {expected}, got {actual}"` per contracts/public-api.md
- [ ] T004 Add `downcast_ref<T: 'static>(&self) -> Result<&T, DowncastError>` method to `AgentMessage` in `src/types.rs` that returns `Ok(&T)` for `Custom` variant when downcast succeeds, or `Err(DowncastError)` with expected/actual type info; returns `Err` for `Llm` variant
- [ ] T005 Add `File` variant to `ImageSource` enum in `src/types.rs` with fields `path: std::path::PathBuf` and `media_type: String` per data-model.md and contracts/public-api.md
- [ ] T006 Add `media_type: String` field to `ImageSource::Url` variant in `src/types.rs` per contracts/public-api.md (currently missing from Url variant)
- [ ] T007 Re-export `DowncastError` from `src/lib.rs` alongside existing `AgentError` re-export

**Checkpoint**: Foundation ready — all spec-required types and methods exist. User story implementation can now begin in parallel.

---

## Phase 3: User Story 1 — Define Message Types for Conversation History (Priority: P1) 🎯 MVP

**Goal**: Verify that all three message types (User, Assistant, ToolResult) carry role-appropriate fields and compose into a conversation sequence with correct serialization round-trips.

**Independent Test**: Construct each message type, compose them into a conversation sequence, serialize/deserialize, verify all fields preserved.

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL or are pending before implementation**

- [ ] T008 [P] [US1] Write integration test `user_message_construction_and_access` in `tests/types.rs` that creates a `UserMessage` with text content and verifies role-appropriate fields (content blocks, timestamp) are accessible
- [ ] T009 [P] [US1] Write integration test `assistant_message_construction_and_access` in `tests/types.rs` that creates an `AssistantMessage` with all fields (content, provider, model_id, usage, cost, stop_reason, error_message, timestamp) and verifies each is accessible
- [ ] T010 [P] [US1] Write integration test `tool_result_message_construction_and_access` in `tests/types.rs` that creates a `ToolResultMessage` with tool_call_id, content, is_error flag, timestamp and verifies each is accessible
- [ ] T011 [P] [US1] Write integration test `message_conversation_sequence` in `tests/types.rs` that composes User → Assistant → ToolResult → Assistant into a `Vec<AgentMessage>` and verifies all messages accessible in order
- [ ] T012 [P] [US1] Write integration test `llm_message_serde_roundtrip` in `tests/types.rs` that serializes and deserializes each `LlmMessage` variant and asserts all fields round-trip with zero data loss

### Implementation for User Story 1

- [ ] T013 [US1] Verify `UserMessage`, `AssistantMessage`, `ToolResultMessage` structs in `src/types.rs` derive `Serialize` and `Deserialize` and all fields match data-model.md (content, timestamp, provider, model_id, usage, cost, stop_reason, error_message, tool_call_id, is_error)
- [ ] T014 [US1] Verify `LlmMessage` enum in `src/types.rs` has correct serde tagging (`#[serde(tag = "role")]`) and all three variants wrap their respective struct types
- [ ] T015 [US1] Verify all US1 tests pass: `cargo test -p swink-agent --test types`

**Checkpoint**: User Story 1 complete — message types are fully functional and tested

---

## Phase 4: User Story 2 — Represent Rich Content Blocks (Priority: P1)

**Goal**: All four content block types (text, thinking, tool call, image) can be constructed, embedded in messages, and pattern-matched.

**Independent Test**: Construct each content block variant, embed in messages, verify correct access and pattern matching.

### Tests for User Story 2

- [ ] T016 [P] [US2] Write integration test `content_block_text_construction` in `tests/types.rs` verifying a text block contains a plain text string
- [ ] T017 [P] [US2] Write integration test `content_block_thinking_with_signature` in `tests/types.rs` verifying a thinking block contains reasoning string and optional verification signature
- [ ] T018 [P] [US2] Write integration test `content_block_tool_call_with_empty_args` in `tests/types.rs` verifying a tool call block with `arguments: serde_json::json!({})` is valid and contains call ID, tool name, parsed arguments, and optional partial buffer
- [ ] T019 [P] [US2] Write integration test `content_block_image_all_sources` in `tests/types.rs` verifying image blocks from all three source types: Base64 (with media_type + data), Url (with url + media_type), and File (with path + media_type)
- [ ] T020 [P] [US2] Write integration test `content_block_serde_roundtrip_all_variants` in `tests/types.rs` that serializes and deserializes each `ContentBlock` variant and asserts exact equality

### Implementation for User Story 2

- [ ] T021 [US2] Verify `ContentBlock` enum in `src/types.rs` has all four variants (Text, Thinking, ToolCall, Image) with correct field types per contracts/public-api.md, including the `#[non_exhaustive]` attribute
- [ ] T022 [US2] Verify `ImageSource` enum in `src/types.rs` has all three variants (Base64, Url, File) with correct field types per data-model.md after T005 and T006
- [ ] T023 [US2] Verify all US2 tests pass: `cargo test -p swink-agent --test types`

**Checkpoint**: User Story 2 complete — all content block types work correctly

---

## Phase 5: User Story 3 — Track Token Usage and Cost (Priority: P1)

**Goal**: Usage and Cost types support independent access to all counters and correct arithmetic aggregation.

**Independent Test**: Create usage/cost records, aggregate across multiple records, verify correct arithmetic.

### Tests for User Story 3

- [ ] T024 [P] [US3] Write integration test `usage_individual_counters` in `tests/types.rs` that creates a Usage record and verifies all counters (input, output, cache_read, cache_write, total) are independently accessible
- [ ] T025 [P] [US3] Write integration test `usage_aggregation_two_records` in `tests/types.rs` that sums two Usage records via `Add` and verifies each counter sums correctly
- [ ] T026 [P] [US3] Write integration test `usage_add_assign` in `tests/types.rs` that uses `AddAssign` to accumulate usage and verifies correctness
- [ ] T027 [P] [US3] Write integration test `usage_zero_counters_valid` in `tests/types.rs` that creates `Usage::default()` and verifies all counters are zero (edge case from spec)
- [ ] T028 [P] [US3] Write integration test `cost_per_category_and_total` in `tests/types.rs` that creates a Cost record with per-category costs, verifies total is sum of all categories via `Add`
- [ ] T029 [P] [US3] Write integration test `cost_add_assign` in `tests/types.rs` that uses `AddAssign` to accumulate cost and verifies correctness
- [ ] T030 [P] [US3] Write integration test `usage_cost_serde_roundtrip` in `tests/types.rs` that serializes and deserializes Usage and Cost records and verifies zero data loss

### Implementation for User Story 3

- [ ] T031 [US3] Verify `Usage` struct in `src/types.rs` has all five counters as `u64`, derives `Default`, and implements `Add` + `AddAssign` with correct arithmetic per data-model.md
- [ ] T032 [US3] Verify `Cost` struct in `src/types.rs` has all five fields as `f64`, derives `Default`, and implements `Add` + `AddAssign` with correct arithmetic per data-model.md
- [ ] T033 [US3] Verify all US3 tests pass: `cargo test -p swink-agent --test types`

**Checkpoint**: User Story 3 complete — usage and cost tracking works correctly

---

## Phase 6: User Story 4 — Handle Errors as Typed Conditions (Priority: P1)

**Goal**: Every error variant is a distinct, matchable type with meaningful description and standard error trait implementation.

**Independent Test**: Construct each error variant, verify contextual data, display message, and trait implementation.

### Tests for User Story 4

- [ ] T034 [P] [US4] Write integration test `error_context_overflow_display` in `tests/error.rs` that constructs `AgentError::ContextWindowOverflow` with a model name and verifies display message contains the model name
- [ ] T035 [P] [US4] Write integration test `error_structured_output_display` in `tests/error.rs` that constructs `AgentError::StructuredOutputFailed` with attempts and last_error and verifies display contains both
- [ ] T036 [P] [US4] Write integration test `error_all_variants_implement_std_error` in `tests/error.rs` that constructs each variant and verifies it implements `std::error::Error` (can be used as `&dyn Error`)
- [ ] T037 [P] [US4] Write integration test `error_retryable_classification` in `tests/error.rs` that verifies `is_retryable()` returns true only for `ModelThrottled` and `NetworkError`, false for all others
- [ ] T038 [P] [US4] Write integration test `error_stream_error_source_chain` in `tests/error.rs` that constructs `AgentError::StreamError` with a source error and verifies `source()` returns the inner error
- [ ] T039 [P] [US4] Write integration test `downcast_error_display` in `tests/error.rs` that constructs `DowncastError` and verifies display message contains expected and actual type names

### Implementation for User Story 4

- [ ] T040 [US4] Verify `AgentError` enum in `src/error.rs` has all spec-required variants (ContextWindowOverflow, ModelThrottled, NetworkError, StructuredOutputFailed, AlreadyRunning, NoMessages, InvalidContinue, StreamError, Aborted) with correct fields and display messages per contracts/public-api.md
- [ ] T041 [US4] Verify `AgentError` derives `thiserror::Error` and all variants implement `std::error::Error` with proper `#[source]` annotations for error chaining
- [ ] T042 [US4] Verify all US4 tests pass: `cargo test -p swink-agent --test error`

**Checkpoint**: User Story 4 complete — error taxonomy is fully functional

---

## Phase 7: User Story 5 — Extend Messages with Application-Specific Types (Priority: P2)

**Goal**: Custom message types can be wrapped, stored in conversation history, downcast back to original type, and are filtered out by the LLM conversion pipeline.

**Independent Test**: Define a custom message type, wrap as AgentMessage, store alongside standard messages, downcast back successfully.

### Tests for User Story 5

- [ ] T043 [P] [US5] Write integration test `custom_message_wrap_and_store` in `tests/types.rs` that defines a custom type implementing `CustomMessage`, wraps it as `AgentMessage::Custom`, stores it in a `Vec<AgentMessage>` alongside `Llm` messages, and verifies it persists
- [ ] T044 [P] [US5] Write integration test `custom_message_downcast_success` in `tests/types.rs` that wraps a custom type, then calls `downcast_ref::<T>()` and verifies the original data is accessible
- [ ] T045 [P] [US5] Write integration test `custom_message_downcast_wrong_type` in `tests/types.rs` that wraps a custom type, calls `downcast_ref::<WrongType>()`, and verifies it returns `Err(DowncastError)` with expected vs actual type info
- [ ] T046 [P] [US5] Write integration test `custom_message_downcast_on_llm_variant` in `tests/types.rs` that calls `downcast_ref::<T>()` on an `AgentMessage::Llm` variant and verifies it returns `Err(DowncastError)`

### Implementation for User Story 5

- [ ] T047 [US5] Verify `CustomMessage` trait in `src/types.rs` requires `Send + Sync + Debug + Any + 'static` bounds and has `as_any()` and `type_name()` methods per contracts/public-api.md
- [ ] T048 [US5] Verify `AgentMessage::downcast_ref` method (from T004) works correctly with `DowncastError` return type and provides expected vs actual type info for debugging
- [ ] T049 [US5] Verify all US5 tests pass: `cargo test -p swink-agent --test types`

**Checkpoint**: User Story 5 complete — custom message extension works correctly

---

## Phase 8: Remaining Types — StopReason, ModelSpec, AgentResult, AgentContext (Cross-Cutting)

**Goal**: Verify all remaining spec types (FR-008 through FR-012) that don't belong to a single user story are correctly implemented and tested.

**Independent Test**: Construct each type, verify fields, serde round-trip, and builder patterns.

### Tests

- [ ] T050 [P] Write integration test `stop_reason_all_variants` in `tests/types.rs` that constructs each `StopReason` variant (Stop, Length, ToolUse, Aborted, Error) and verifies pattern matching and serde round-trip (FR-008)
- [ ] T051 [P] Write integration test `thinking_level_all_variants` in `tests/types.rs` that constructs each `ThinkingLevel` variant (Off, Minimal, Low, Medium, High, ExtraHigh) and verifies Default is Off and serde round-trip (FR-010)
- [ ] T052 [P] Write integration test `model_spec_construction_and_builder` in `tests/types.rs` that creates a `ModelSpec` via `new()` and chains `with_thinking_level()`, `with_thinking_budgets()`, `with_provider_config()`, `with_capabilities()` and verifies all fields (FR-009)
- [ ] T053 [P] Write integration test `model_spec_serde_roundtrip` in `tests/types.rs` that serializes and deserializes a fully-populated `ModelSpec` and verifies zero data loss (FR-009)
- [ ] T054 [P] Write integration test `agent_result_construction` in `tests/types.rs` that creates an `AgentResult` with messages, stop_reason, usage, cost, and optional error, then verifies all fields are accessible (FR-011)
- [ ] T055 [P] Write integration test `agent_context_construction` in `tests/types.rs` that creates an `AgentContext` with system_prompt, messages, and tools vector, then verifies all fields accessible (FR-012)

### Verification

- [ ] T056 Verify `StopReason` enum in `src/types.rs` has all five variants with `#[non_exhaustive]`, derives `PartialEq, Eq, Hash, Serialize, Deserialize`
- [ ] T057 Verify `ModelSpec` struct in `src/types.rs` has `new()` constructor and `with_*()` builder methods per contracts/public-api.md
- [ ] T058 Verify all Phase 8 tests pass: `cargo test -p swink-agent --test types`

**Checkpoint**: All spec types verified and tested

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Final verification, compile-time checks, and comprehensive test suite validation

- [ ] T059 [P] Verify compile-time `Send + Sync` assertions in `src/types.rs` cover all public types including `DowncastError` (FR-015)
- [ ] T060 [P] Verify all public types are re-exported from `src/lib.rs` so consumers access them via `use swink_agent::*` per contracts/public-api.md
- [ ] T061 Run full test suite: `cargo test --workspace` and verify zero failures
- [ ] T062 Run clippy: `cargo clippy --workspace -- -D warnings` and verify zero warnings
- [ ] T063 Run quickstart.md validation: verify all code examples from `specs/002-foundation-types-errors/quickstart.md` compile and produce expected output

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Stories (Phase 3–7)**: All depend on Foundational phase completion
  - US1 (Messages), US2 (Content Blocks), US3 (Usage/Cost), US4 (Errors): Independent, can proceed in parallel (all P1)
  - US5 (Custom Messages): Depends on T003/T004 from Phase 2 (DowncastError + downcast_ref)
- **Remaining Types (Phase 8)**: Can run in parallel with user stories (after Phase 2)
- **Polish (Phase 9)**: Depends on all previous phases being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational (Phase 2) — no dependencies on other stories
- **User Story 2 (P1)**: Can start after Foundational (Phase 2) — no dependencies on other stories; uses ImageSource variants added in Phase 2
- **User Story 3 (P1)**: Can start after Foundational (Phase 2) — no dependencies on other stories
- **User Story 4 (P1)**: Can start after Foundational (Phase 2) — uses DowncastError from Phase 2
- **User Story 5 (P2)**: Can start after Foundational (Phase 2) — uses downcast_ref + DowncastError from Phase 2

### Within Each User Story

- Tests are written first per phase ordering
- For scaffolded code, "verify" tasks confirm existing behavior (not strict red-green-refactor)
- Verify tasks confirm alignment with spec contracts
- Story complete before moving to next priority

### Parallel Opportunities

- All P1 user stories (US1–US4) can start in parallel after Phase 2
- Phase 8 (Remaining Types) can run in parallel with user stories
- All test tasks marked [P] within a story can run in parallel
- T003–T007 in Phase 2 can run in parallel (different types, different files)

---

## Parallel Example: User Story 1

```bash
# Launch all tests for User Story 1 together:
Task: T008 "Write user_message_construction_and_access test in tests/types.rs"
Task: T009 "Write assistant_message_construction_and_access test in tests/types.rs"
Task: T010 "Write tool_result_message_construction_and_access test in tests/types.rs"
Task: T011 "Write message_conversation_sequence test in tests/types.rs"
Task: T012 "Write llm_message_serde_roundtrip test in tests/types.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup — verify scaffold
2. Complete Phase 2: Foundational — add DowncastError, File variant, media_type on Url
3. Complete Phase 3: User Story 1 — message types
4. **STOP and VALIDATE**: `cargo test -p swink-agent --test types`
5. Proceed to remaining stories

### Incremental Delivery

1. Complete Setup + Foundational → Foundation ready
2. Add User Story 1 → Test → Message types verified (MVP!)
3. Add User Stories 2–4 in parallel → All P1 stories complete
4. Add User Story 5 → Custom message extension verified
5. Add Phase 8 → All remaining types verified
6. Polish → Full verification pass

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Existing scaffold code means most "implementation" tasks are verification + alignment tasks
- Existing inline tests in `src/types.rs` cover content blocks, usage/cost arithmetic, model spec, and custom messages — new integration tests in `tests/types.rs` focus on spec acceptance criteria and any gaps
- Key spec gaps to fill: DowncastError, ImageSource::File variant, ImageSource::Url media_type field
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
