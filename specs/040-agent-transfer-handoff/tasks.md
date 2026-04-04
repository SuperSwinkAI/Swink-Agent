# Tasks: TransferToAgent Tool & Handoff Safety

**Input**: Design documents from `/specs/040-agent-transfer-handoff/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Included — the project constitution mandates test-driven development (tests before implementation).

**Organization**: Tasks grouped by user story for independent implementation and testing.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup

**Purpose**: Feature gate and module scaffolding

- [x] T001 Add `transfer` feature flag to `Cargo.toml` (default-enabled) in root `Cargo.toml`
- [x] T002 Create `src/transfer.rs` with `#[cfg(feature = "transfer")]` module gate and re-export in `src/lib.rs`

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core type modifications that ALL user stories depend on — StopReason::Transfer, AgentToolResult.transfer_signal, AgentResult.transfer_signal

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T003 [P] Add `StopReason::Transfer` unit variant to the StopReason enum in `src/types/mod.rs`
- [x] T004 [P] Create `TransferSignal` struct with `target_agent`, `reason`, `context_summary`, `conversation_history` fields, constructors (`new()`, `with_context_summary()`, `with_conversation_history()`), accessors, and derives (Clone, Debug, Serialize, Deserialize) in `src/transfer.rs`
- [x] T005 Add `transfer_signal: Option<TransferSignal>` field to `AgentToolResult` in `src/tool.rs` with `#[serde(default, skip_serializing_if = "Option::is_none")]`, add `transfer()` constructor and `is_transfer()` method
- [x] T006 Add `transfer_signal: Option<TransferSignal>` field to `AgentResult` in `src/types/mod.rs` with `#[serde(default, skip_serializing_if = "Option::is_none")]`
- [x] T007 Write unit tests for `TransferSignal` constructors, accessors, and serde round-trip in `src/transfer.rs`
- [x] T008 Write unit tests for `AgentToolResult::transfer()` constructor and `is_transfer()` method in `src/tool.rs`
- [x] T009 Write test that existing `AgentToolResult::text()` and `error()` constructors produce `transfer_signal: None` in `src/tool.rs`
- [x] T010 Write test that `AgentToolResult` JSON deserialization without `transfer_signal` field defaults to None (backward compat) in `src/tool.rs`

**Checkpoint**: Core types compile, all existing tests pass, new foundational tests pass. `cargo test -p swink-agent` green.

---

## Phase 3: User Story 1 - Agent Transfers Conversation (Priority: P1) MVP

**Goal**: LLM calls `transfer_to_agent` tool, loop terminates with transfer signal containing target, reason, and conversation history.

**Independent Test**: Configure agent with transfer tool, have LLM call it, verify loop terminates with StopReason::Transfer and TransferSignal in AgentResult.

### Tests for US1

- [ ] T011 [US1] Write test: TransferToAgentTool validates target exists in registry, returns transfer signal on success in `src/transfer.rs`
- [ ] T012 [US1] Write test: TransferToAgentTool returns error result when target agent not in registry in `src/transfer.rs`
- [ ] T013 [US1] Write test: TransferToAgentTool includes context_summary in signal when provided in `src/transfer.rs`
- [ ] T014 [US1] Write test: agent loop detects transfer signal in tool results, terminates turn with StopReason::Transfer, and enriches signal with conversation history in `src/loop_/turn.rs`
- [ ] T015 [US1] Write test: tool result text is "Transfer to {agent_name} initiated." on success in `src/transfer.rs`

### Implementation for US1

- [ ] T016 [US1] Implement `TransferToAgentTool` struct with `registry: Arc<AgentRegistry>` and `allowed_targets: Option<HashSet<String>>` fields, `new(registry)` constructor in `src/transfer.rs`
- [ ] T017 [US1] Implement `AgentTool` trait for `TransferToAgentTool` — `name()`, `description()`, `schema()` (JSON schema with agent_name, reason, context_summary params), `execute()` validates target in registry, returns `AgentToolResult::transfer(signal)` with confirmation text in `src/transfer.rs`
- [ ] T018 [US1] Add transfer signal detection in agent loop after tool results are collected — scan results for `is_transfer()`, if found enrich signal with conversation history from `LoopState.context_messages`, set `StopReason::Transfer` and break inner loop in `src/loop_/turn.rs`

**Checkpoint**: Basic transfer works end-to-end. `cargo test -p swink-agent` passes.

---

## Phase 4: User Story 2 - Restricted Allowed Targets (Priority: P1)

**Goal**: Transfer tool rejects attempts to agents not in the configured allowed set.

**Independent Test**: Create transfer tool with allowed_targets, attempt transfer to allowed agent (succeeds) and disallowed agent (error result).

### Tests for US2

- [ ] T019 [US2] Write test: transfer to allowed target succeeds in `src/transfer.rs`
- [ ] T020 [US2] Write test: transfer to disallowed target returns error result with clear message in `src/transfer.rs`
- [ ] T021 [US2] Write test: unrestricted tool (allowed_targets: None) allows transfer to any registered agent in `src/transfer.rs`
- [ ] T022 [US2] Write test: empty allowed_targets set rejects all transfers in `src/transfer.rs`

### Implementation for US2

- [ ] T023 [US2] Implement `with_allowed_targets(registry, targets)` constructor on `TransferToAgentTool` in `src/transfer.rs`
- [ ] T024 [US2] Add allowed_targets validation in `execute()` — check target against set before registry lookup, return error result if not allowed in `src/transfer.rs`

**Checkpoint**: Allowed targets restriction works. `cargo test -p swink-agent` passes.

---

## Phase 5: User Story 3 - Circular Transfer Detection (Priority: P1)

**Goal**: TransferChain prevents infinite handoff loops by detecting circular references and depth limit violations.

**Independent Test**: Create chain, push A then B, verify pushing A again returns CircularTransfer error.

### Tests for US3

- [ ] T025 [US3] Write test: TransferChain rejects circular transfer (agent already in chain) in `src/transfer.rs`
- [ ] T026 [US3] Write test: TransferChain rejects when max_depth exceeded in `src/transfer.rs`
- [ ] T027 [US3] Write test: TransferChain allows push of new agent not in chain in `src/transfer.rs`
- [ ] T028 [US3] Write test: TransferChain::default() has max_depth 5 in `src/transfer.rs`
- [ ] T029 [US3] Write test: TransferChain::contains() and depth() return correct values in `src/transfer.rs`
- [ ] T030 [US3] Write test: self-transfer is always circular (current agent is first in chain) in `src/transfer.rs`

### Implementation for US3

- [ ] T031 [US3] Implement `TransferChain` struct with `chain: Vec<String>` and `max_depth: usize`, `new(max_depth)`, `Default` (max_depth: 5), `push()`, `depth()`, `contains()`, `chain()` in `src/transfer.rs`
- [ ] T032 [US3] Implement `TransferError` enum with CircularTransfer and MaxDepthExceeded variants, Display, Error in `src/transfer.rs`

**Checkpoint**: Circular detection works. `cargo test -p swink-agent` passes.

---

## Phase 6: User Story 4 - Transfer Events (Priority: P2)

**Goal**: Emit events for transfer activity via existing event system.

**Independent Test**: Register event listener, trigger transfer, verify listener receives transfer-requested event.

### Tests for US4

- [ ] T033 [US4] Write test: transfer-requested event emitted when transfer tool succeeds in `src/loop_/turn.rs`
- [ ] T034 [US4] Write test: transfer-rejected event emitted when transfer tool returns error (target not found or not allowed) in `src/loop_/turn.rs`

### Implementation for US4

- [ ] T035 [US4] Add transfer event emission in the loop's transfer detection path — emit transfer-requested event (via `AgentEvent::Custom(Emission)`) when transfer signal found, emit transfer-rejected when transfer tool returns error in `src/loop_/turn.rs`

**Checkpoint**: Transfer events work. `cargo test -p swink-agent` passes.

---

## Phase 7: User Story 5 - Orchestration Context (Priority: P2)

**Goal**: Transfer signal carries sufficient context for consumers to build orchestration patterns.

**Independent Test**: Trigger transfer, inspect signal, verify target, reason, summary, and full conversation history are present.

### Tests for US5

- [ ] T036 [US5] Write test: AgentResult with StopReason::Transfer has transfer_signal containing target, reason, context_summary in `src/transfer.rs`
- [ ] T037 [US5] Write test: transfer signal conversation_history contains all LLM messages from agent session (custom messages filtered out) in `src/loop_/turn.rs`
- [ ] T038 [US5] Write test: conversation_history includes tool results from concurrent tool calls in the same turn in `src/loop_/turn.rs`

### Implementation for US5

- [ ] T039 [US5] Ensure loop enrichment filters custom messages from conversation_history (include only AgentMessage::Llm variants), consistent with existing `in_flight_llm_messages` pattern in `src/loop_/turn.rs`

**Checkpoint**: Full orchestration context works. `cargo test -p swink-agent` passes.

---

## Phase 8: Edge Cases (Priority: P1)

**Purpose**: Handle multi-transfer and cancellation edge cases from spec

### Tests

- [ ] T040 Write test: only first transfer signal is honored when LLM calls transfer_to_agent multiple times in one turn in `src/loop_/turn.rs`
- [ ] T041 Write test: cancellation token takes precedence over transfer — loop returns Aborted, not Transfer in `src/loop_/turn.rs`
- [ ] T042 Write test: transfer tool alongside other tools — all tool results processed, transfer terminates turn after in `src/loop_/turn.rs`

### Implementation

- [ ] T043 Implement multi-transfer deduplication in loop — take first signal, log warning for duplicates in `src/loop_/turn.rs`

**Checkpoint**: All edge cases handled. `cargo test -p swink-agent` passes.

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Final quality and verification

- [ ] T044 [P] Add doc comments to all public types and methods in `src/transfer.rs`
- [ ] T045 [P] Add `Send + Sync` compile-time assertions for `TransferToAgentTool`, `TransferSignal`, `TransferChain` in `src/transfer.rs`
- [ ] T046 Run `cargo clippy -p swink-agent -- -D warnings` and fix any warnings
- [ ] T047 Run `cargo test --workspace` final pass — all tests green including downstream crates
- [ ] T048 Verify `cargo test -p swink-agent --no-default-features` compiles (transfer module gated)
- [ ] T049 Run `cargo build --workspace` to verify no workspace-level breakage

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 — BLOCKS all user stories
- **US1 Transfer (Phase 3)**: Depends on Phase 2 — MVP target
- **US2 Allowed Targets (Phase 4)**: Depends on Phase 3 (extends transfer tool)
- **US3 Circular Detection (Phase 5)**: Depends on Phase 2 only — can run parallel with US1
- **US4 Events (Phase 6)**: Depends on Phase 3 (needs loop integration)
- **US5 Orchestration (Phase 7)**: Depends on Phase 3 (needs working transfer)
- **Edge Cases (Phase 8)**: Depends on Phase 3
- **Polish (Phase 9)**: Depends on all desired phases

### User Story Dependencies

- **US1 (Transfer)**: Can start after Foundational — MVP target
- **US2 (Allowed Targets)**: Depends on US1 (extends tool execute())
- **US3 (Circular Detection)**: Independent of US1/US2 — can run parallel with US1
- **US4 (Events)**: Depends on US1 (needs loop integration point)
- **US5 (Orchestration)**: Depends on US1 (needs working transfer result)

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Types before logic
- Core implementation before integration

### Parallel Opportunities

- T003, T004 (StopReason variant and TransferSignal) can run in parallel
- T007, T008, T009, T010 (foundational tests) can run in parallel
- US3 (circular detection) can run parallel with US1 (transfer)
- T044, T045 (polish) can run in parallel

---

## Parallel Example: Foundational Types

```bash
# Launch independent foundational type changes:
Task: "T003 - Add StopReason::Transfer in src/types/mod.rs"
Task: "T004 - Create TransferSignal in src/transfer.rs"
```

## Parallel Example: After Foundational

```bash
# US1 and US3 can start in parallel:
Task: "US1 - Transfer tool + loop integration"
Task: "US3 - TransferChain circular detection"
```

---

## Implementation Strategy

### MVP First (US1)

1. Complete Phase 1: Setup (feature gate, module)
2. Complete Phase 2: Foundational types (StopReason, TransferSignal, AgentToolResult/AgentResult fields)
3. Complete Phase 3: US1 (TransferToAgentTool + loop integration)
4. **STOP and VALIDATE**: Transfer works end-to-end
5. `cargo test -p swink-agent` all green

### Incremental Delivery

1. Setup + Foundational → Core types ready
2. Add US1 (Transfer) → MVP — basic handoff works
3. Add US2 (Allowed Targets) → Access control
4. Add US3 (Circular Detection) → Safety — all P1 stories complete
5. Add US4 (Events) → Observability
6. Add US5 (Orchestration Context) → Full context
7. Edge Cases + Polish → Production ready

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Constitution requires TDD — tests written before implementation in each story phase
- StopReason::Transfer is a unit variant (preserves Copy) — signal data in AgentResult.transfer_signal
- Tool returns partial signal, loop enriches with conversation history
- Feature gate covers src/transfer.rs types only — StopReason variant and result fields are always compiled
