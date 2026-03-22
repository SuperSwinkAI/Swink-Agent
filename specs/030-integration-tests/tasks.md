# Tasks: Integration Tests

**Input**: Design documents from `/specs/030-integration-tests/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Extend shared test helpers and create test file scaffolding

- [ ] T001 Extract and enhance `EventCollector` from existing `tests/integration.rs` into `tests/common/mod.rs` — wraps `Arc<Mutex<Vec<AgentEvent>>>`, provides `new()`, `subscriber()` (returns `impl Fn(&AgentEvent)` closure for `agent.subscribe()`), `events()` (snapshot clone), and `count()` methods per contracts/public-api.md
- [ ] T002 Add helper function `tool_call_events_multi(calls: &[(&str, &str, &str)]) -> Vec<AssistantMessageEvent>` to `tests/common/mod.rs` for building multi-tool-call response event sequences (Start, N tool call blocks, Done with ToolUse stop reason)
- [ ] T003 Add helper function `error_events(message: &str, error_kind: Option<StreamErrorKind>) -> Vec<AssistantMessageEvent>` to `tests/common/mod.rs` for building error response event sequences (Start, Error with configurable kind)

**Checkpoint**: Shared helpers ready — all test files can use `EventCollector` and extended helpers

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Create the six test file skeletons with `mod common;` and required imports

**NOTE**: No foundational blocking prerequisites beyond Phase 1. Test files are independent and created in their respective user story phases.

**Checkpoint**: Foundation ready — user story implementation can now begin in parallel

---

## Phase 3: User Story 1 — Verify Core Agent Lifecycle and Events (Priority: P1)

**Goal**: Confirm the agent starts, processes messages, emits lifecycle events in order, and accumulates history across turns.

**Independent Test**: `cargo test --test ac_lifecycle`

### Implementation for User Story 1

- [ ] T004 [US1] Create test file `tests/ac_lifecycle.rs` with `mod common;` and imports for `swink_agent::{Agent, AgentOptions, AgentEvent}`, `MockStreamFn`, `EventCollector`, and helpers
- [ ] T005 [US1] Implement AC 1 test `agent_creation_with_mock_stream` in `tests/ac_lifecycle.rs` — create an Agent with `AgentOptions::new("prompt", default_model(), mock_stream_fn, default_convert)`, send a user message via `agent.prompt_async(vec![...])`, and assert a text response is returned
- [ ] T006 [US1] Implement AC 2 test `message_processing_produces_response` in `tests/ac_lifecycle.rs` — send a user message via `agent.prompt_async()`, collect the returned response text, and assert it matches the scripted mock stream output
- [ ] T007 [US1] Implement AC 3 test `lifecycle_events_emitted_in_order` in `tests/ac_lifecycle.rs` — attach an `EventCollector` via `agent.subscribe()`, send a message, and assert events arrive in order: TurnStart, streaming events, TurnEnd
- [ ] T008 [US1] Implement AC 4 test `streaming_delivers_text_tokens` in `tests/ac_lifecycle.rs` — attach an `EventCollector`, send a message with a multi-token scripted response (multiple TextDelta events), and assert all text deltas are received and concatenated correctly
- [ ] T009 [US1] Implement AC 5 test `turn_completion_accumulates_history` in `tests/ac_lifecycle.rs` — send two sequential messages, and assert the agent's context contains both user messages and both assistant responses after the second turn
- [ ] T010 [US1] Implement edge case test `panicking_subscriber_is_removed` in `tests/ac_lifecycle.rs` — register a subscriber that panics on first event, register a second `EventCollector` subscriber, send a message, and assert the panicking subscriber was auto-removed while the second subscriber received all events

**Checkpoint**: AC 1–5 passing. `cargo test --test ac_lifecycle` succeeds independently.

---

## Phase 4: User Story 2 — Verify Tool Execution and Validation (Priority: P1)

**Goal**: Confirm tools are registered, validated against schema, executed (including concurrently), and results flow back into the conversation.

**Independent Test**: `cargo test --test ac_tools`

### Implementation for User Story 2

- [ ] T011 [US2] Create test file `tests/ac_tools.rs` with `mod common;` and imports for `swink_agent::{Agent, AgentOptions, AgentTool, AgentToolResult}`, `MockStreamFn`, `MockTool`, `tool_call_events`, and helpers
- [ ] T012 [US2] Implement AC 6 test `tool_registration_and_discovery` in `tests/ac_tools.rs` — register a `MockTool` via `AgentOptions`, script a tool call in the mock stream, and assert the tool's `execute()` was called (via `was_executed()`)
- [ ] T013 [US2] Implement AC 7 test `schema_validation_rejects_invalid_args` in `tests/ac_tools.rs` — register a `MockTool` with a strict schema (e.g. requiring a `"path"` string property), script a tool call with invalid args (missing required field), and assert the tool was NOT executed and an error result was returned
- [ ] T014 [US2] Implement AC 8 test `tool_execution_with_valid_args` in `tests/ac_tools.rs` — register a `MockTool`, script a tool call with valid args matching the schema, and assert the tool was executed and its result text appears in the follow-up context
- [ ] T015 [US2] Implement AC 9 test `concurrent_tool_execution` in `tests/ac_tools.rs` — register three `MockTool` instances each with a delay (e.g. 50ms), script a response with all three tool calls in a single turn using `tool_call_events_multi`, capture start `Instant` per tool, and assert all three tools were executed (via `execution_count`) and that start times are within epsilon of each other (proving concurrency per research.md:D3)
- [ ] T016 [US2] Implement AC 10 test `tool_error_handling` in `tests/ac_tools.rs` — register a `MockTool` configured with `AgentToolResult::error("something failed")`, script a tool call, and assert the error result is returned to the agent and the loop continues (agent produces a final text response)
- [ ] T017 [US2] Implement AC 11 test `tool_result_in_followup_message` in `tests/ac_tools.rs` — register a `MockTool`, script a tool call followed by a text response, use `ContextCapturingStreamFn` to capture context on the second call, and assert the tool result message is present in the captured context
- [ ] T018 [US2] Implement AC 12 test `tool_call_transformation` in `tests/ac_tools.rs` — configure a `ToolCallTransformer` on `AgentOptions` that modifies tool call arguments (e.g. adds a field), register a `MockTool`, script a tool call, and assert the transformer was invoked and the modified arguments reached the tool
- [ ] T019 [US2] Implement edge case test `tool_validator_rejects_call` in `tests/ac_tools.rs` — configure a `ToolValidator` on `AgentOptions` that rejects a specific tool call, register a `MockTool`, script a tool call, and assert the tool was NOT executed and a rejection error result was returned

**Checkpoint**: AC 6–12 passing. `cargo test --test ac_tools` succeeds independently.

---

## Phase 5: User Story 3 — Verify Context Management and Overflow (Priority: P1)

**Goal**: Confirm sliding window compaction preserves anchor/tail, tool-result pairs stay together, and overflow triggers retry.

**Independent Test**: `cargo test --test ac_context`

### Implementation for User Story 3

- [ ] T020 [US3] Create test file `tests/ac_context.rs` with `mod common;` and imports for `swink_agent::{Agent, AgentOptions, ContextWindowConfig}`, `ContextCapturingStreamFn`, and helpers
- [ ] T021 [US3] Implement AC 13 test `context_window_tracking` in `tests/ac_context.rs` — configure a small `ContextWindowConfig` (e.g. max_tokens = 500), send multiple messages to accumulate history, and use `ContextCapturingStreamFn` to assert the message count passed to the stream decreases after compaction triggers
- [ ] T022 [US3] Implement AC 14 test `sliding_window_preserves_anchor_and_tail` in `tests/ac_context.rs` — configure a small context budget, send enough messages to trigger compaction, capture context via `ContextCapturingStreamFn`, and assert the first message (anchor) and most recent messages (tail) are preserved while middle messages are removed
- [ ] T023 [US3] Implement AC 15 test `context_overflow_triggers_retry` in `tests/ac_context.rs` — configure a very small context budget, script the mock stream to return the `CONTEXT_OVERFLOW_SENTINEL` error on the first call and a normal text response on the second call (after compaction), and assert the agent recovers and returns a successful response
- [ ] T024 [US3] Implement AC 16 test `tool_result_pairs_kept_together` in `tests/ac_context.rs` — send messages that include a tool call and its result, trigger compaction, and assert the tool call message and its corresponding tool result are either both kept or both removed (never split)
- [ ] T025 [US3] Implement edge case test `transform_context_callback_on_overflow` in `tests/ac_context.rs` — configure a `transform_context` callback on `AgentOptions`, trigger overflow, and assert the callback is invoked with the context for custom compaction

**Checkpoint**: AC 13–16 passing. `cargo test --test ac_context` succeeds independently.

---

## Phase 6: User Story 4 — Verify Retry, Steering, and Abort (Priority: P2)

**Goal**: Confirm retry with backoff works, steering callbacks modify messages between turns, abort stops mid-turn, and the sync API blocks correctly.

**Independent Test**: `cargo test --test ac_resilience`

### Implementation for User Story 4

- [ ] T026 [US4] Create test file `tests/ac_resilience.rs` with `mod common;` and imports for `swink_agent::{Agent, AgentOptions, AgentError}`, `MockStreamFn`, `EventCollector`, and helpers
- [ ] T027 [US4] Implement AC 17 test `retry_with_backoff_on_throttle` in `tests/ac_resilience.rs` — script the mock stream to return a `ModelThrottled` error on the first call and a successful text response on the second, configure a retry strategy, and assert the agent eventually succeeds
- [ ] T028 [US4] Implement AC 18 test `steering_callback_modifies_messages` in `tests/ac_resilience.rs` — use `agent.steer(AgentMessage::user("injected"))` to inject a message between turns, script a multi-turn conversation (tool call then follow-up), and assert the injected steering message appears in the context
- [ ] T029 [US4] Implement AC 19 test `abort_stops_running_turn` in `tests/ac_resilience.rs` — start a message with a mock stream that includes a delay, cancel via `CancellationToken` mid-turn, and assert the agent returns with an aborted/cancelled status
- [ ] T030 [US4] Implement AC 20 test `sync_api_blocks_until_complete` in `tests/ac_resilience.rs` — use `agent.prompt_sync()` (or equivalent sync API), send a message, and assert it blocks and returns the complete response (test runs on a separate thread via `std::thread::spawn`)
- [ ] T031 [US4] Implement AC 21 test `followup_decision_controls_continuation` in `tests/ac_resilience.rs` — configure a `should_continue` callback on `AgentOptions` that returns `false` after the first turn, script a tool call response (which normally triggers follow-up), and assert the agent stops after one turn instead of continuing
- [ ] T032 [US4] Implement AC 22 test `custom_messages_survive_compaction` in `tests/ac_resilience.rs` — pass an `AgentMessage::Custom` via `agent.prompt_async()` input vector or `agent.steer()`, trigger compaction, capture context via `ContextCapturingStreamFn`, and assert the custom message is still present in context but was NOT sent to the provider (filtered by `default_convert` returning `None`)

**Checkpoint**: AC 17–22 passing. `cargo test --test ac_resilience` succeeds independently.

---

## Phase 7: User Story 5 — Verify Structured Output and Proxy Reconstruction (Priority: P2)

**Goal**: Confirm structured output with schema enforcement works, and proxy stream event sequences reconstruct faithfully.

**Independent Test**: `cargo test --test ac_structured`

### Implementation for User Story 5

- [ ] T033 [US5] Create test file `tests/ac_structured.rs` with `mod common;` and imports for `swink_agent::{Agent, AgentOptions}`, `swink_agent_adapters::proxy::ProxyStreamFn`, `MockStreamFn`, `serde_json::json!`, and helpers
- [ ] T034 [US5] Implement AC 23 test `structured_output_with_schema` in `tests/ac_structured.rs` — use `agent.structured_output(prompt, schema)` with a JSON schema Value (e.g. `json!({"type": "object", "properties": {"name": {"type": "string"}}})`), script the mock stream to return valid JSON matching the schema, and assert the response is parsed as structured output
- [ ] T035 [US5] Implement AC 24 test `schema_enforcement_rejects_invalid` in `tests/ac_structured.rs` — use `agent.structured_output(prompt, schema)` with a schema, script the mock stream to return JSON that does NOT match the schema, and assert the agent retries or returns an appropriate error
- [ ] T036 [US5] Implement AC 25 test `proxy_stream_reconstruction` in `tests/ac_structured.rs` — create a `ProxyStreamFn`, feed it serialized event data (Start, TextDelta, Done), stream the events, and assert the reconstructed event sequence matches the original
- [ ] T037 [US5] Implement edge case test `structured_output_empty_object` in `tests/ac_structured.rs` — configure structured output with a permissive schema, script the mock stream to return `{}`, and assert empty object is accepted

**Checkpoint**: AC 23–25 passing. `cargo test --test ac_structured` succeeds independently.

---

## Phase 8: User Story 6 — Verify TUI Rendering and Interaction (Priority: P3)

**Goal**: Confirm TUI components render correctly with role-based colors, inline diffs, context gauge thresholds, plan mode restrictions, and approval mode classification.

**Independent Test**: `cargo test -p swink-agent-tui --test ac_tui`

### Implementation for User Story 6

- [ ] T038 [US6] Create test file `tui/tests/ac_tui.rs` (inside the TUI crate, NOT the core crate) with imports for `swink_agent_tui` types (`App`, `TuiConfig`), `ratatui::backend::TestBackend`, `ratatui::Terminal`, and TUI-internal theme/rendering modules accessible from within the crate
- [ ] T039 [US6] Implement AC 26 test `role_based_border_colors` in `tui/tests/ac_tui.rs` — create `DisplayMessage` instances with different `MessageRole` values (User, Assistant, System), render them using the conversation view into a `TestBackend` buffer, and assert each role's block has the correct border color from the theme module
- [ ] T040 [US6] Implement AC 27 test `inline_diff_color_coding` in `tui/tests/ac_tui.rs` — create a `DiffData` struct with known old/new content, call `render_diff_lines()`, and assert additions are styled with `theme::diff_add_color()`, removals with `theme::diff_remove_color()`, and context with `theme::diff_context_color()`
- [ ] T041 [US6] Implement AC 28 test `context_gauge_color_thresholds` in `tui/tests/ac_tui.rs` — render the status bar or context gauge at different utilization percentages (30% = green, 70% = yellow, 90% = red), and assert the gauge color matches the expected threshold color
- [ ] T042 [US6] Implement AC 29 test `plan_mode_restricts_write_tools` in `tui/tests/ac_tui.rs` — create an `App` in plan mode, verify the plan mode indicator is set, and assert that write tools (e.g. `WriteFileTool`) are filtered out or unavailable while read tools remain
- [ ] T043 [US6] Implement AC 30 test `approval_mode_classifies_tools` in `tui/tests/ac_tui.rs` — create an `App` with `ApprovalMode::Smart`, and assert that tools with `requires_approval = false` (read tools) auto-execute while tools with `requires_approval = true` (write tools) trigger an approval prompt

**Checkpoint**: AC 26–30 passing. `cargo test --test ac_tui` succeeds independently.

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Final validation and cleanup across all test files

- [ ] T044 Run `cargo test --test ac_lifecycle --test ac_tools --test ac_context --test ac_resilience --test ac_structured --test ac_tui` and fix any compilation or test failures
- [ ] T045 Run `cargo clippy --workspace -- -D warnings` and fix any new warnings introduced by the test files
- [ ] T046 Run `cargo test --workspace` to ensure new tests do not break any existing tests
- [ ] T047 Verify all 30 acceptance criteria are covered by grepping test function names across all `ac_*.rs` files and cross-referencing against data-model.md AC mapping table
- [ ] T048 Run quickstart.md validation — execute the commands from `specs/030-integration-tests/quickstart.md` and confirm they work as documented

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Phase 1 (EventCollector and helpers)
- **User Stories (Phases 3–8)**: All depend on Phase 1 completion
  - US1, US2, US3 (P1 stories) can proceed in parallel
  - US4, US5 (P2 stories) can proceed in parallel, independent of P1 stories
  - US6 (P3 story) can proceed independently
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: No dependencies on other stories. Uses `EventCollector` from Phase 1.
- **User Story 2 (P1)**: No dependencies on other stories. Uses `MockTool`, `tool_call_events` from Phase 1.
- **User Story 3 (P1)**: No dependencies on other stories. Uses `ContextCapturingStreamFn` from Phase 1.
- **User Story 4 (P2)**: No dependencies on other stories. Uses `MockStreamFn`, `EventCollector` from Phase 1.
- **User Story 5 (P2)**: No dependencies on other stories. Uses `MockStreamFn`, `ProxyStreamFn` from adapters crate.
- **User Story 6 (P3)**: No dependencies on other stories. Uses `swink-agent-tui` dev-dependency.

### Within Each User Story

- Create test file skeleton first (imports, `mod common;`)
- Implement tests sequentially within the file (each test is independent but file must compile)
- All tests within a story are parallelizable at runtime (`cargo test` runs them in parallel)

### Parallel Opportunities

- All Phase 1 tasks (T001–T003) can run in parallel (different additions to same file, but logically independent)
- All six user story phases (Phases 3–8) can run in parallel after Phase 1 completes
- Within each user story, tests for different ACs are independent and can be written in parallel if the file skeleton exists

---

## Parallel Example: User Story 2

```
# After Phase 1 completes, launch US2 file creation:
Task T011: Create tests/ac_tools.rs skeleton

# Then implement all AC tests in parallel (different functions, same file):
Task T012: AC 6 — tool_registration_and_discovery
Task T013: AC 7 — schema_validation_rejects_invalid_args
Task T014: AC 8 — tool_execution_with_valid_args
Task T015: AC 9 — concurrent_tool_execution
Task T016: AC 10 — tool_error_handling
Task T017: AC 11 — tool_result_in_followup_message
Task T018: AC 12 — tool_call_transformation
Task T019: Edge case — tool_validator_rejects_call
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (T001–T003)
2. Complete Phase 3: User Story 1 (T004–T010)
3. **STOP and VALIDATE**: `cargo test --test ac_lifecycle`
4. AC 1–5 verified

### Incremental Delivery

1. Phase 1 (Setup) — helpers ready
2. Phase 3 (US1: Lifecycle) — AC 1–5 verified
3. Phase 4 (US2: Tools) — AC 6–12 verified
4. Phase 5 (US3: Context) — AC 13–16 verified
5. Phase 6 (US4: Resilience) — AC 17–22 verified
6. Phase 7 (US5: Structured) — AC 23–25 verified
7. Phase 8 (US6: TUI) — AC 26–30 verified
8. Phase 9 (Polish) — full suite validated

### Parallel Team Strategy

With multiple developers after Phase 1:

- Developer A: US1 (lifecycle) + US4 (resilience) — both use EventCollector
- Developer B: US2 (tools) + US3 (context) — both focus on core agent mechanics
- Developer C: US5 (structured) + US6 (TUI) — both cover advanced features

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable via `cargo test --test ac_<name>`
- All tests use shared infrastructure from `tests/common/mod.rs`
- No external services, network, or API keys required for any test
- Existing `tests/integration.rs` is left untouched — new tests complement it
