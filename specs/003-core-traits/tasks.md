# Tasks: Core Traits

**Input**: Design documents from `/specs/003-core-traits/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Tests**: Included — the spec explicitly requires tests for all acceptance scenarios (SC-001 through SC-007).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

**Context**: The existing codebase contains reference implementations of all three traits (`AgentTool`, `StreamFn`, `RetryStrategy`) with tests. These tasks audit the existing code against the spec contracts, fill coverage gaps, document deviations, and ensure public API correctness.

**TDD Note**: Constitution principle II mandates tests-before-implementation. Since this is a verification spec (existing code, not green-field), tasks follow an audit→verify→add-missing-tests pattern rather than strict red-green-refactor.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Verify workspace prerequisites and establish spec-contract alignment baseline

- [x] T001 Verify feature 002 types are available: run `cargo build -p swink-agent` and confirm ContentBlock, AgentContext, ModelSpec, Usage, Cost, StopReason, AgentError, AssistantMessage compile from src/types.rs and src/error.rs
- [x] T002 [P] Verify workspace dependencies are available: confirm jsonschema, tokio-util (CancellationToken), futures (Stream), rand, and serde_json are in workspace Cargo.toml
- [x] T003 [P] Create spec-contract deviation log at specs/003-core-traits/deviations.md documenting known differences between the spec contracts (contracts/public-api.md) and the existing implementation (field name differences, type differences, extra methods)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Audit and align common infrastructure used by all three user stories

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 Audit src/tool.rs AgentToolResult struct against spec contract: spec says `details: Option<Value>`, existing uses `details: Value`. Document deviation in specs/003-core-traits/deviations.md — existing design is intentional (Value::Null serves as None)
- [x] T005 [P] Audit src/stream.rs AssistantMessageEvent enum field names against spec contract: spec uses `index`/`text`/`thinking`/`json_fragment`, existing uses `content_index`/`delta` uniformly. Document deviation in specs/003-core-traits/deviations.md — existing design is intentional (uniform field names simplify accumulation)
- [x] T006 [P] Audit src/stream.rs StreamOptions against spec contract: spec says `max_tokens: Option<u32>`, existing uses `Option<u64>`. Existing also adds `api_key: Option<String>` not in spec. Document deviation in specs/003-core-traits/deviations.md
- [x] T007 [P] Audit src/retry.rs RetryStrategy trait against spec contract: spec says `attempt: usize`, existing uses `attempt: u32`. Existing also adds `as_any()` method. Document deviation in specs/003-core-traits/deviations.md
- [x] T008 [P] Audit src/stream.rs AssistantMessageEvent::Start variant: spec includes `{ provider, model }` fields, existing uses unit variant `Start` with provider/model passed to accumulate_message instead. Document deviation in specs/003-core-traits/deviations.md
- [x] T009 Audit src/lib.rs re-exports: verify all public API items from spec contracts are re-exported — AgentTool, AgentToolResult, validate_tool_arguments, StreamFn, StreamOptions, AssistantMessageEvent, AssistantMessageDelta, accumulate_message, RetryStrategy, DefaultRetryStrategy, StreamTransport, StreamErrorKind

**Checkpoint**: Foundation audit complete — all deviations documented, re-exports verified

---

## Phase 3: User Story 1 — Implement a Custom Tool (Priority: P1) 🎯 MVP

**Goal**: Verify the AgentTool trait, AgentToolResult, and validate_tool_arguments match all acceptance scenarios from spec

**Independent Test**: Implement a mock tool, validate args against schema, execute with valid/invalid args, verify structured results

### Tests for User Story 1

> **NOTE: Verify existing tests cover all acceptance scenarios; add missing tests**

- [x] T010 [US1] Audit tests/tool.rs against acceptance scenario 1 (valid args → execute called → structured result): verify existing `mock_tool_executes` test covers this. Confirm result contains content blocks with correct data
- [x] T011 [US1] Audit tests/tool.rs against acceptance scenario 2 (invalid args → rejected with field-level errors without calling execute): verify existing `invalid_type_produces_errors` and `missing_required_field_caught` tests cover this
- [x] T012 [P] [US1] Audit tests/tool.rs against acceptance scenario 3 (missing required fields caught): verify existing `empty_object_missing_required_field` test covers this
- [x] T013 [P] [US1] Audit tests/tool.rs against acceptance scenario 4 (result contains content blocks + optional details + is_error flag): verify existing `text_result_constructor` and `error_result_constructor` tests cover this. Add test for `is_error` flag distinction if missing

### Implementation for User Story 1

- [x] T014 [US1] Verify AgentTool trait in src/tool.rs has all spec-required methods: name(), label(), description(), parameters_schema(), execute() with correct signatures. Confirm trait is object-safe (Arc<dyn AgentTool> works). Verify execute accepts update callback parameter (FR-002) and that callback can receive streaming partial results
- [x] T015 [US1] Verify AgentToolResult in src/tool.rs has convenience constructors: text() (is_error=false) and error() (is_error=true). Confirm is_error flag behavior matches spec FR-005
- [x] T016 [US1] Verify validate_tool_arguments in src/tool.rs handles edge cases: empty schema accepts empty args (FR-003), field-level errors are descriptive
- [x] T017 [US1] Add integration test in tests/tool.rs: implement a tool with empty parameter schema (edge case from spec), verify empty args `{}` are accepted
- [x] T018 [US1] Add test in tests/tool.rs verifying is_error flag: AgentToolResult::text() sets is_error=false, AgentToolResult::error() sets is_error=true (SC-001)

**Checkpoint**: User Story 1 fully verified — mock tool validates, executes, returns structured results

---

## Phase 4: User Story 2 — Plug in an LLM Provider (Priority: P1)

**Goal**: Verify StreamFn trait, StreamOptions, AssistantMessageEvent protocol, and accumulate_message match all acceptance scenarios

**Independent Test**: Implement a mock stream emitting scripted events, verify accumulation produces correct AssistantMessage

### Tests for User Story 2

- [x] T019 [US2] Audit tests/stream.rs against acceptance scenario 1 (text deltas → complete text block): verify existing `accumulate_text_and_tool_call` test covers this
- [x] T020 [US2] Audit tests/stream.rs against acceptance scenario 2 (interleaved text + tool call deltas → both assembled correctly): verify existing `accumulate_interleaved_text_and_tool_calls` test covers this (SC-007)
- [x] T021 [P] [US2] Audit tests/stream.rs against acceptance scenario 3 (done event with usage → finalized message carries statistics): verify existing test checks usage.input, usage.output, cost.total
- [x] T022 [P] [US2] Audit tests/stream.rs against acceptance scenario 4 (error event → message carries error + stop_reason): verify existing `accumulate_error_event` test covers this
- [x] T023 [P] [US2] Audit tests/stream.rs against acceptance scenario 5 (stream options defaults): verify existing `stream_options_defaults` test covers this

### Implementation for User Story 2

- [x] T024 [US2] Verify StreamFn trait in src/stream.rs is object-safe with correct signature: accepts ModelSpec, AgentContext, StreamOptions, CancellationToken, returns Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>>
- [x] T025 [US2] Verify AssistantMessageEvent enum in src/stream.rs has all spec-required variants: Start, TextStart/Delta/End, ThinkingStart/Delta/End, ToolCallStart/Delta/End, Done, Error
- [x] T026 [US2] Verify accumulate_message in src/stream.rs enforces strict ordering (FR-011): one Start, indexed content blocks, one terminal. Out-of-order events return error (SC-002)
- [x] T027 [US2] Verify accumulate_message handles ToolCallEnd with empty partial JSON → `{}` not null (FR-012): existing `accumulate_tool_call_empty_args` test confirms this
- [x] T028 [US2] Add test in tests/stream.rs for empty stream (zero events) edge case from spec: verify it returns error "no Start event found"
- [x] T029 [P] [US2] Add test in tests/stream.rs for AssistantMessageDelta enum: verify all three variants (Text, Thinking, ToolCall) can be constructed with correct fields
- [x] T030 [US2] Verify AssistantMessageEvent convenience constructors in src/stream.rs: error(), error_throttled(), error_context_overflow(), error_auth(), error_network() — confirm existing unit tests in src/stream.rs cover these

**Checkpoint**: User Story 2 fully verified — mock stream accumulates correctly, ordering enforced, edge cases handled

---

## Phase 5: User Story 3 — Configure Retry Behavior (Priority: P2)

**Goal**: Verify RetryStrategy trait and DefaultRetryStrategy match all acceptance scenarios

**Independent Test**: Simulate retryable/non-retryable errors, verify correct retry decisions and delay computation

### Tests for User Story 3

- [x] T031 [US3] Audit tests/retry.rs against acceptance scenario 1 (rate-limit on first attempt → retry): verify existing `retries_model_throttled_up_to_max_attempts` test covers this
- [x] T032 [P] [US3] Audit tests/retry.rs against acceptance scenario 2 (rate-limit on max attempt → no retry): verify existing test checks attempt == max_attempts returns false
- [x] T033 [P] [US3] Audit tests/retry.rs against acceptance scenario 3 (context overflow → no retry): verify existing `does_not_retry_context_window_overflow` test covers this
- [x] T034 [P] [US3] Audit tests/retry.rs against acceptance scenario 4 (delays increase exponentially and cap): verify existing `delay_increases_exponentially_without_jitter` and `delay_caps_at_max_delay` tests cover this (SC-004)
- [x] T035 [P] [US3] Audit tests/retry.rs against acceptance scenario 5 (jitter varies delays within [0.5, 1.5)): verify existing `jitter_produces_varying_delays` test covers this (SC-005)

### Implementation for User Story 3

- [x] T036 [US3] Verify RetryStrategy trait in src/retry.rs: should_retry(error, attempt) and delay(attempt) methods. Confirm trait is object-safe (Box<dyn RetryStrategy>)
- [x] T037 [US3] Verify DefaultRetryStrategy in src/retry.rs: max_attempts=3, base_delay=1s, max_delay=60s, multiplier=2.0, jitter=true defaults (FR-014)
- [x] T038 [US3] Verify DefaultRetryStrategy retries ONLY ModelThrottled and NetworkError (FR-015): confirm does_not_retry_non_retryable_variants test covers Aborted, StreamError, StructuredOutputFailed, ContextWindowOverflow (SC-003)
- [x] T039 [US3] Add test in tests/retry.rs verifying jitter range is within [0.5, 1.5) of computed delay (FR-016): compute delay without jitter, then verify 100 jittered samples fall within [0.5×base, 1.5×base]. Also verify clamping to max_delay happens BEFORE jitter is applied (contract behavioral rule)
- [x] T040 [US3] Verify DefaultRetryStrategy builder methods in src/retry.rs: with_max_attempts(), with_base_delay(), with_max_delay(), with_multiplier(), with_jitter() — existing `builder_methods` test covers this
- [x] T041 [US3] Add test in tests/retry.rs for acceptance scenario 6 (custom retry strategy replaces default): implement a custom RetryStrategy that retries all errors, verify it can be used as Box<dyn RetryStrategy>

**Checkpoint**: User Story 3 fully verified — retry decisions correct, delays bounded, jitter within range, custom strategy works

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Final validation, build, and cleanup

- [x] T042 [P] Run `cargo test -p swink-agent` and verify all existing + new tests pass
- [x] T043 [P] Run `cargo clippy -p swink-agent -- -D warnings` and verify zero warnings
- [x] T044 [P] Run `cargo test -p swink-agent --no-default-features` and verify builtin-tools feature gate works
- [x] T045 Finalize specs/003-core-traits/deviations.md with complete deviation summary and rationale for each
- [x] T046 Update specs/003-core-traits/spec.md status from "Draft" to "Verified"
- [x] T047 Run quickstart.md validation: verify all code examples in specs/003-core-traits/quickstart.md are consistent with the actual API

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Stories (Phase 3–5)**: All depend on Foundational phase completion
  - US1 and US2 can proceed in parallel (different files: tool.rs vs stream.rs)
  - US3 can proceed in parallel with US1/US2 (different file: retry.rs)
- **Polish (Phase 6)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational (Phase 2) — No dependencies on other stories
- **User Story 2 (P1)**: Can start after Foundational (Phase 2) — No dependencies on other stories
- **User Story 3 (P2)**: Can start after Foundational (Phase 2) — No dependencies on other stories

### Within Each User Story

- Audit existing tests against acceptance scenarios first
- Verify implementation against spec contracts
- Add any missing tests identified during audit
- Story complete before moving to Polish phase

### Parallel Opportunities

- T002 and T003 can run in parallel (Setup phase)
- T004, T005, T006, T007, T008 can all run in parallel (Foundational audit — different concerns)
- All three user stories (Phase 3, 4, 5) can run in parallel — each targets a different source file
- T042, T043, T044 can run in parallel (Polish — independent verification commands)

---

## Parallel Example: All User Stories

```bash
# After Phase 2 completes, launch all three stories in parallel:
# Story 1 (tool.rs):
Task: "Verify AgentTool trait in src/tool.rs"
Task: "Add test for empty parameter schema edge case"

# Story 2 (stream.rs):
Task: "Verify StreamFn trait in src/stream.rs"
Task: "Add test for empty stream edge case"

# Story 3 (retry.rs):
Task: "Verify RetryStrategy trait in src/retry.rs"
Task: "Add test for jitter range validation"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: User Story 1 (AgentTool)
4. **STOP and VALIDATE**: Run `cargo test -p swink-agent` — verify tool tests pass
5. Proceed to remaining stories

### Incremental Delivery

1. Complete Setup + Foundational → Deviation log established
2. Add User Story 1 → Test independently → Tool trait verified
3. Add User Story 2 → Test independently → Stream trait verified
4. Add User Story 3 → Test independently → Retry trait verified
5. Polish → All tests pass, clippy clean, quickstart validated

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together
2. Once Foundational is done:
   - Developer A: User Story 1 (src/tool.rs, tests/tool.rs)
   - Developer B: User Story 2 (src/stream.rs, tests/stream.rs)
   - Developer C: User Story 3 (src/retry.rs, tests/retry.rs)
3. Stories complete and integrate independently

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable
- Existing code is the reference implementation — tasks verify spec alignment, not green-field creation
- Deviations between spec contracts and implementation are expected and must be documented with rationale
- Stop at any checkpoint to validate story independently
