# Tasks: Agent Loop

**Input**: Design documents from `/specs/004-agent-loop/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Tests**: Tests are included as the spec explicitly lists test files and the project follows test-driven development principles (CLAUDE.md: "Test-driven. Run `cargo test --workspace` before every commit.").

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization — create the module structure, types, and event emission infrastructure shared by all user stories.

- [ ] T001 [P] Create loop module directory structure with `src/loop_/mod.rs` defining `AgentLoopConfig`, `LoopState`, `TurnEndReason`, `AgentEvent` types, and module declarations for `turn`, `stream`, `tool_dispatch`
- [ ] T002 [P] Define callback type aliases (`ConvertToLlmFn`, `TransformContextFn`, `GetApiKeyFn`, `GetSteeringMessagesFn`, `GetFollowUpMessagesFn`) in `src/loop_/mod.rs`
- [ ] T003 Implement event emission helper using `tokio::mpsc` channel in `src/emit.rs` — send `AgentEvent` values to the stream consumer
- [ ] T004 Update `src/lib.rs` to re-export public API: `agent_loop`, `agent_loop_continue`, `AgentLoopConfig`, `AgentEvent`, `TurnEndReason`, and all callback type aliases

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core loop skeleton and stream infrastructure that MUST be complete before any user story can be implemented.

**CRITICAL**: No user story work can begin until this phase is complete.

- [ ] T005 Implement `agent_loop` entry point in `src/loop_/mod.rs` — prepend prompt messages, validate context (NoMessages check), emit `AgentStart`, spawn inner loop, return `impl Stream<Item = AgentEvent>`
- [ ] T006 Implement `agent_loop_continue` entry point in `src/loop_/mod.rs` — validate last message is not assistant (InvalidContinue check), validate non-empty, delegate to shared loop body
- [ ] T007 Implement outer loop skeleton in `src/loop_/mod.rs` — create the outer/inner loop structure with placeholders; `AgentEnd` emission is wired in T016
- [ ] T008 Implement single-turn skeleton in `src/loop_/turn.rs` — create function signatures and control flow structure (`TurnStart`/`TurnEnd` emission) that compiles but delegates to placeholder calls; real logic wired in T013–T015
- [ ] T009 Implement stream invocation in `src/loop_/stream.rs` — call `StreamFn` with converted messages, accumulate `AssistantMessage` from stream events, emit `MessageStart`/`MessageUpdate`/`MessageEnd`

**Checkpoint**: Foundation ready — loop can execute a single turn with mock provider returning text only

---

## Phase 3: User Story 1 — Single-Turn Conversation (Priority: P1)

**Goal**: A developer sends a prompt and receives a streamed response with lifecycle events in correct order: AgentStart, TurnStart, MessageStart, MessageUpdate(s), MessageEnd, TurnEnd, AgentEnd.

**Independent Test**: Mock LLM provider returns a scripted text response; verify the event sequence matches the expected lifecycle.

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T010 [P] [US1] Write test for lifecycle event ordering (AgentStart → TurnStart → MessageStart → MessageUpdate(s) → MessageEnd → TurnEnd → AgentEnd) in `tests/loop_single_turn.rs`
- [ ] T011 [P] [US1] Write test verifying `AgentEnd` carries produced messages in `tests/loop_single_turn.rs`
- [ ] T012 [P] [US1] Write test verifying `TurnEnd` carries assistant message with `TurnEndReason::Complete` on natural stop in `tests/loop_single_turn.rs`
- [ ] T012a [P] [US1] Write test verifying `ConvertToLlmFn` returning `None` for a custom message correctly filters it from the provider call (FR-016) in `tests/loop_single_turn.rs`
- [ ] T012b [P] [US1] Write test verifying `get_api_key` is called before each `StreamFn` invocation and the resolved key is passed through (FR-006) in `tests/loop_single_turn.rs`

### Implementation for User Story 1

- [ ] T013 [US1] Implement complete single-turn flow in `src/loop_/turn.rs` — wire `transform_context` → `convert_to_llm` → `get_api_key` → `StreamFn` → accumulate → emit lifecycle events in correct order
- [ ] T014 [US1] Implement identity transform behavior when `transform_context` is `None` in `src/loop_/turn.rs`
- [ ] T015 [US1] Wire `TurnEnd` event with `TurnEndReason::Complete` and assistant message when no tool calls in `src/loop_/turn.rs`
- [ ] T016 [US1] Wire `AgentEnd` event with all produced messages in `src/loop_/mod.rs`

**Checkpoint**: Single-turn no-tool conversation emits all lifecycle events in correct order (SC-001)

---

## Phase 4: User Story 2 — Multi-Turn Tool Execution (Priority: P1)

**Goal**: Agent receives tool calls, executes them concurrently, collects results, injects into context, and re-invokes provider until no more tool calls.

**Independent Test**: Mock provider requests tool calls on turn 1, returns text on turn 2; verify tool execution events and multi-turn event sequence.

### Tests for User Story 2

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T017 [P] [US2] Write test for tool execution events (ToolExecutionStart, ToolExecutionEnd) emitted for each tool call in `tests/loop_tool_execution.rs`
- [ ] T018 [P] [US2] Write test verifying concurrent tool execution (not sequential) and each tool receives a distinct child `CancellationToken` (FR-008) in `tests/loop_tool_execution.rs`
- [ ] T019 [P] [US2] Write test verifying tool results are injected into context and provider is called again in `tests/loop_tool_execution.rs`
- [ ] T020 [P] [US2] Write test verifying loop exits normally when provider stops requesting tools in `tests/loop_tool_execution.rs`

### Implementation for User Story 2

- [ ] T021 [US2] Implement tool call detection in `src/loop_/turn.rs` — extract tool calls from accumulated assistant message after stream completes
- [ ] T022 [US2] Implement concurrent tool dispatch in `src/loop_/tool_dispatch.rs` — `tokio::spawn` per tool call with per-tool child `CancellationToken`, emit `ToolExecutionStart`/`ToolExecutionEnd` events
- [ ] T023 [US2] Implement tool result collection and context injection in `src/loop_/tool_dispatch.rs` — collect all results, create `ToolResultMessage` entries, append to context messages
- [ ] T024 [US2] Wire inner loop continuation in `src/loop_/mod.rs` — when tools executed, set `TurnEndReason::ToolsExecuted` and loop back for next turn
- [ ] T025 [US2] Implement `ToolExecutionUpdate` event emission for intermediate tool output in `src/loop_/tool_dispatch.rs`

**Checkpoint**: Multi-turn tool cycles work — concurrent execution verified (SC-002)

---

## Phase 5: User Story 3 — Steering Interrupts (Priority: P1)

**Goal**: During tool execution, caller injects steering message; loop cancels remaining tools, injects error results for cancelled tools, and processes steering on next turn.

**Independent Test**: Mock provider with slow tools, inject steering mid-execution; verify remaining tools cancelled and steering message processed.

### Tests for User Story 3

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T026 [P] [US3] Write test verifying steering cancels remaining in-flight tools via cancellation tokens in `tests/loop_steering.rs`
- [ ] T027 [P] [US3] Write test verifying cancelled tools get error results indicating steering interrupt in `tests/loop_steering.rs`
- [ ] T028 [P] [US3] Write test verifying steering message is processed on next turn in `tests/loop_steering.rs`

### Implementation for User Story 3

- [ ] T029 [US3] Implement steering message polling after each tool completion in `src/loop_/tool_dispatch.rs` — call `get_steering_messages` callback after each tool finishes
- [ ] T030 [US3] Implement tool cancellation on steering arrival in `src/loop_/tool_dispatch.rs` — cancel remaining child tokens, inject error result for each cancelled tool
- [ ] T031 [US3] Implement steering message queuing in `src/loop_/mod.rs` — add steering messages to `LoopState.pending_messages`, process as pending before next provider call
- [ ] T032 [US3] Wire `TurnEndReason::SteeringInterrupt` in `src/loop_/turn.rs` when steering interrupt occurs
- [ ] T033 [US3] Handle steering messages when no tools are executing — queue as pending messages in `LoopState` for next turn in `src/loop_/turn.rs`

**Checkpoint**: Steering interrupts cancel remaining tools and redirect agent (SC-003)

---

## Phase 6: User Story 4 — Follow-Up Continuation (Priority: P2)

**Goal**: When the agent would stop naturally, the loop checks for follow-up messages and continues if available.

**Independent Test**: Follow-up callback returns messages on first poll, nothing on second; verify loop continues then stops.

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T034 [P] [US4] Write test verifying follow-up messages cause loop continuation in `tests/loop_follow_up.rs`
- [ ] T035 [P] [US4] Write test verifying loop emits `AgentEnd` when no follow-up messages in `tests/loop_follow_up.rs`
- [ ] T036 [P] [US4] Write test verifying error/abort exits skip follow-up polling in `tests/loop_follow_up.rs`

### Implementation for User Story 4

- [ ] T037 [US4] Implement follow-up polling in outer loop in `src/loop_/mod.rs` — call `get_follow_up_messages` when inner loop exits normally (Complete)
- [ ] T038 [US4] Implement follow-up message injection in `src/loop_/mod.rs` — add follow-up messages to context, re-enter inner loop
- [ ] T039 [US4] Implement error/abort guard in outer loop in `src/loop_/mod.rs` — skip follow-up polling on Error or Aborted exit, emit `AgentEnd` immediately (FR-011)

**Checkpoint**: Follow-up continuation works; error/abort exits skip follow-up (SC-004, SC-005)

---

## Phase 7: User Story 5 — Error Recovery and Retry (Priority: P2)

**Goal**: Transient provider errors trigger retry strategy; the loop recovers on success or surfaces the error when retries are exhausted.

**Independent Test**: Mock provider fails on first call, succeeds on second; verify retry strategy consulted and loop recovers.

### Tests for User Story 5

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T040 [P] [US5] Write test verifying retryable error triggers retry with delay in `tests/loop_retry.rs`
- [ ] T041 [P] [US5] Write test verifying exhausted retries surface error and exit in `tests/loop_retry.rs`
- [ ] T042 [P] [US5] Write test verifying non-retryable error causes immediate exit in `tests/loop_retry.rs`

### Implementation for User Story 5

- [ ] T043 [US5] Implement retry integration in `src/loop_/stream.rs` — on retryable error, call `RetryStrategy::should_retry()`, compute delay with jitter, re-invoke `StreamFn`
- [ ] T044 [US5] Implement retry exhaustion handling in `src/loop_/stream.rs` — when retries exhausted, surface error via `AgentEvent::TurnEnd` with `TurnEndReason::Error`
- [ ] T045 [US5] Implement non-retryable error fast path in `src/loop_/stream.rs` — exit immediately without consulting retry strategy

**Checkpoint**: Retry integration works for transient failures (SC-006, SC-007)

---

## Phase 8: User Story 6 — Context Overflow Recovery (Priority: P2)

**Goal**: Provider rejects request due to context overflow; loop signals overflow condition to transformation hook and retries with reduced context.

**Independent Test**: Mock provider rejects with overflow on first call, succeeds on second; verify overflow signal passed to transformation hook.

### Tests for User Story 6

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T046 [P] [US6] Write test verifying overflow error sets `overflow_signal = true` on `LoopState` in `tests/loop_overflow.rs`
- [ ] T047 [P] [US6] Write test verifying `transform_context` receives overflow signal and reduced context is used on retry in `tests/loop_overflow.rs`

### Implementation for User Story 6

- [ ] T048 [US6] Implement overflow detection in `src/loop_/stream.rs` — detect `CONTEXT_OVERFLOW_SENTINEL` from provider error, set `LoopState.overflow_signal = true`
- [ ] T049 [US6] Implement overflow signal passing in `src/loop_/turn.rs` — pass `overflow_signal` to `transform_context`, reset to `false` after call; when `transform_context` returns `Some(report)`, emit `ContextCompacted` event
- [ ] T050 [US6] Implement overflow retry loop in `src/loop_/mod.rs` — when overflow detected, re-enter turn with overflow signal set (not a retry strategy error, a loop control signal)

**Checkpoint**: Context overflow triggers transformation hook with overflow signal (SC-008)

---

## Phase 9: User Story 7 — Max Tokens Recovery (Priority: P3)

**Goal**: Provider stops mid-response with stop reason "length" and incomplete tool calls; loop replaces each incomplete tool call with an error result and continues.

**Independent Test**: Mock provider returns length-limited response with partial tool calls; verify error results injected and loop continues.

### Tests for User Story 7

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T051 [P] [US7] Write test verifying incomplete tool calls are replaced with error results on stop reason "length" in `tests/loop_max_tokens.rs`
- [ ] T052 [P] [US7] Write test verifying loop continues after max tokens recovery in `tests/loop_max_tokens.rs`

### Implementation for User Story 7

- [ ] T053 [US7] Implement max tokens detection in `src/loop_/turn.rs` — detect stop reason "length" with incomplete tool calls from accumulated message
- [ ] T054 [US7] Implement incomplete tool call replacement in `src/loop_/turn.rs` — replace each incomplete tool call with an error `AgentToolResult` containing informative message
- [ ] T055 [US7] Wire max tokens recovery into inner loop in `src/loop_/mod.rs` — after replacement, continue loop so agent sees error results and can adjust

**Checkpoint**: Incomplete tool calls from max tokens are replaced and loop continues (SC-009)

---

## Phase 10: Polish & Cross-Cutting Concerns

**Purpose**: Cancellation support, edge cases, and cross-cutting improvements.

- [ ] T056 Write cancellation test verifying clean shutdown with `TurnEndReason::Aborted` in `tests/loop_cancellation.rs`
- [ ] T057 Implement cooperative cancellation via `CancellationToken` in `src/loop_/mod.rs` and `src/loop_/turn.rs` — check token at turn boundaries and during stream, emit `Aborted` stop reason (SC-010)
- [ ] T058 Handle edge case: provider returns zero content blocks (empty response) in `src/loop_/turn.rs` — treat as natural stop with empty assistant message
- [ ] T059 Handle edge case: all tool calls cancelled by steering in `src/loop_/tool_dispatch.rs` — ensure `TurnEnd` is still emitted with `SteeringInterrupt` reason
- [ ] T060 Verify `transform_context` is called before `convert_to_llm` on every turn (SC-011) — add assertion test in `tests/loop_single_turn.rs`
- [ ] T061 Run `cargo clippy -p swink-agent -- -D warnings` and fix any warnings across all new files
- [ ] T062 Run `cargo test -p swink-agent` and verify all tests pass
- [ ] T063 Run quickstart.md verification checklist — verify all 9 items pass

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Stories (Phases 3-9)**: All depend on Foundational phase completion
  - US1 (P1), US2 (P1), US3 (P1): Can proceed in priority order; US2 depends on US1 (tool dispatch builds on single-turn); US3 depends on US2 (steering builds on tool dispatch)
  - US4 (P2): Depends on US1 (follow-up builds on outer loop exit)
  - US5 (P2): Depends on US1 (retry builds on stream invocation)
  - US6 (P2): Depends on US5 (overflow builds on retry/error handling)
  - US7 (P3): Depends on US2 (max tokens builds on tool call handling)
- **Polish (Phase 10)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (Single-Turn)**: After Foundational — no story dependencies
- **US2 (Tool Execution)**: After US1 — extends turn with tool dispatch
- **US3 (Steering)**: After US2 — extends tool dispatch with interrupts
- **US4 (Follow-Up)**: After US1 — extends outer loop with continuation
- **US5 (Retry)**: After US1 — extends stream invocation with retry
- **US6 (Overflow)**: After US5 — extends retry with overflow signaling
- **US7 (Max Tokens)**: After US2 — extends tool call handling with recovery

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Stream/turn infrastructure before dispatch logic
- Core implementation before edge cases
- Story complete before moving to next priority

### Parallel Opportunities

- T001, T002 can run in parallel (same file but distinct sections)
- T003 can run in parallel with T001/T002 (different file: `src/emit.rs`)
- Within each user story, test tasks marked [P] can run in parallel
- US4 (Follow-Up) and US5 (Retry) can run in parallel after US1 (different files, independent concerns)
- US7 (Max Tokens) can run in parallel with US4/US5/US6 after US2 (independent concern)

---

## Parallel Example: User Story 2

```bash
# Launch all tests for User Story 2 together:
Task T017: "Test tool execution events in tests/loop_tool_execution.rs"
Task T018: "Test concurrent tool execution in tests/loop_tool_execution.rs"
Task T019: "Test tool result injection in tests/loop_tool_execution.rs"
Task T020: "Test loop exit on no more tools in tests/loop_tool_execution.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001-T004)
2. Complete Phase 2: Foundational (T005-T009)
3. Complete Phase 3: User Story 1 (T010-T016)
4. **STOP and VALIDATE**: Single-turn conversation emits correct lifecycle events
5. Run `cargo test -p swink-agent` to confirm

### Incremental Delivery

1. Setup + Foundational → Loop skeleton ready
2. US1 (Single-Turn) → Test independently → MVP!
3. US2 (Tool Execution) → Test independently → Agentic behavior works
4. US3 (Steering) → Test independently → Interactive interrupts work
5. US4 (Follow-Up) → Test independently → Autonomous continuation works
6. US5 (Retry) → Test independently → Production reliability
7. US6 (Overflow) → Test independently → Long conversation recovery
8. US7 (Max Tokens) → Test independently → Edge case recovery
9. Polish → All edge cases, clippy clean, full test suite green

### Critical Path

```
Setup → Foundational → US1 → US2 → US3
                         ├──→ US4
                         ├──→ US5 → US6
                         └──→ (after US2) US7
```

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Source files already exist in `src/loop_/` — tasks modify existing files
- The loop is stateless — all state passed via `AgentLoopConfig` and `AgentContext`
- `loop_` uses trailing underscore convention since `loop` is a reserved word
- Test helpers (`MockStreamFn`, `MockTool`, etc.) live in `tests/common/mod.rs`
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
