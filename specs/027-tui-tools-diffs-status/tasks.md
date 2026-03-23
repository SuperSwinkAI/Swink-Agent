# Tasks: TUI: Tool Panel, Diffs & Status Bar

**Input**: Design documents from `/specs/027-tui-tools-diffs-status/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Theme colors, format helpers, and shared types needed by all user stories

- [x] T001 [P] Add diff and status bar color constants (`diff_add_color`, `diff_remove_color`, `diff_context_color`, `status_idle`, `status_running`, `status_error`, `status_aborted`, `context_green`, `context_yellow`, `context_red`, `plan_color`, `bar_bg`, `bar_fg`) to `tui/src/theme.rs`
- [x] T002 [P] Implement `format_tokens(n: u64) -> String` in `tui/src/format.rs` — below 1K as-is, 1K–10K one decimal, 10K–1M rounded, 1M+ with M suffix
- [x] T003 [P] Implement `format_elapsed(start: Instant) -> String` in `tui/src/format.rs` — MM:SS under 1h, HH:MM:SS at or above 1h
- [x] T004 [P] Implement `format_context_gauge(tokens_used: u64, budget: u64) -> (String, f32)` in `tui/src/format.rs` — 10-char bar with fill/empty blocks, returns percentage; budget=0 returns `("[ no limit ]", 0.0)`
- [x] T005 [P] Add unit tests for `format_tokens` boundary values (0, 999, 1000, 4600, 10000, 999999, 1000000, 1500000) in `tui/src/format.rs`
- [x] T006 [P] Add unit tests for `format_elapsed` (0s, 59s, 60s, 3599s, 3600s, 7261s) in `tui/src/format.rs`
- [x] T007 [P] Add unit tests for `format_context_gauge` (budget=0, 0%, 50%, 60%, 85%, 100%, >100%) in `tui/src/format.rs`
- [x] T008 Add `AgentStatus` enum (Idle, Running, Error, Aborted) to `tui/src/app/state.rs` with display text and color mapping
- [x] T009 Extend `DisplayMessage` in `tui/src/app/state.rs` with `collapsed: bool`, `summary: String`, `user_expanded: bool`, `expanded_at: Option<Instant>`, `diff_data: Option<DiffData>` fields
- [x] T010 Add `tool_panel: ToolPanel`, `total_input_tokens: u64`, `total_output_tokens: u64`, `total_cost: f64`, `retry_attempt: Option<u32>`, `context_budget: u64`, `context_tokens_used: u64`, `selected_tool_block: Option<usize>` fields to `App` struct in `tui/src/app/state.rs`

**Checkpoint**: All shared types, format helpers, and theme colors are in place. All format helper unit tests pass.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core data types that MUST exist before any user story UI can be built

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T011 Create `DiffData` struct with `path`, `is_new_file`, `old_content`, `new_content` fields and `from_details(details: &Value) -> Option<Self>` constructor in `tui/src/ui/diff.rs`
- [x] T012 Create `ToolExecution` struct with `id`, `name`, `started_at`, `completed_at`, `is_error` fields in `tui/src/ui/tool_panel.rs`
- [x] T013 Create `PendingApproval` struct with `id`, `name`, `arguments_summary` fields in `tui/src/ui/tool_panel.rs`
- [x] T014 Create `ResolvedApproval` struct with `approved`, `resolved_at` fields in `tui/src/ui/tool_panel.rs`
- [x] T015 Create `ToolPanel` struct with `active`, `completed`, `pending_approvals`, `resolved_approvals`, `spinner_frame` fields and `const fn new() -> Self` constructor in `tui/src/ui/tool_panel.rs`
- [x] T016 Register `pub mod tool_panel;`, `pub mod diff;`, `pub mod status_bar;` in `tui/src/ui/mod.rs` and `pub mod format;` in `tui/src/lib.rs`
- [x] T017 Add unit test for `DiffData::from_details` with valid JSON, missing fields, and wrong types in `tui/src/ui/diff.rs`

**Checkpoint**: Foundation ready — user story implementation can now begin in parallel

---

## Phase 3: User Story 1 — Monitor Tool Execution in Real Time (Priority: P1) 🎯 MVP

**Goal**: Tool panel docked above conversation shows active tools with animated spinners, completed tools with success/failure badges, and auto-hides after timeout.

**Independent Test**: Trigger agent tool calls, verify spinners appear for active tools and badges on completion. Panel auto-hides 10s after all tools complete.

### Implementation for User Story 1

- [x] T018 [US1] Implement `ToolPanel::start_tool(&mut self, id: String, name: String)` — creates `ToolExecution` and pushes to `active` in `tui/src/ui/tool_panel.rs`
- [x] T019 [US1] Implement `ToolPanel::end_tool(&mut self, id: &str, is_error: bool)` — moves tool from `active` to `completed`, sets `completed_at` and `is_error` in `tui/src/ui/tool_panel.rs`
- [x] T020 [US1] Implement `ToolPanel::set_awaiting_approval(&mut self, id: &str, name: &str, arguments: &Value)` — creates `PendingApproval` with redacted argument summary using `swink_agent::redact_sensitive_values` in `tui/src/ui/tool_panel.rs`
- [x] T021 [US1] Implement `ToolPanel::resolve_approval(&mut self, id: &str, approved: bool)` — removes from `pending_approvals`, adds `ResolvedApproval` in `tui/src/ui/tool_panel.rs`
- [x] T022 [US1] Implement `ToolPanel::tick(&mut self)` — advance `spinner_frame` (mod 10), prune completed entries older than 10s, prune resolved approvals older than 2s in `tui/src/ui/tool_panel.rs`
- [x] T023 [US1] Implement `ToolPanel::is_visible(&self) -> bool` and `ToolPanel::has_pending_approval(&self) -> bool` — `is_visible` returns true if any of the four collections is non-empty in `tui/src/ui/tool_panel.rs`
- [x] T024 [US1] Implement `ToolPanel::height(&self) -> u16` — 0 when hidden, min(total_entries + 2 borders, 10) when visible in `tui/src/ui/tool_panel.rs`
- [x] T025 [US1] Implement `ToolPanel::render(&self, frame: &mut Frame, area: Rect)` — render active tools with braille spinner (SPINNER constant ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏), completed tools with ✓/✗ badge, pending approvals with ⚠ icon, resolved approvals with brief text in `tui/src/ui/tool_panel.rs`
- [x] T026 [US1] Update layout in `tui/src/ui/mod.rs` to allocate conditional tool panel region (0 or `tool_panel.height()` lines) above conversation area
- [x] T027 [US1] Wire `AgentEvent::ToolCallStart` and `AgentEvent::ToolCallEnd` events from agent bridge to `tool_panel.start_tool()`/`end_tool()` in `tui/src/app/agent_bridge.rs`
- [x] T028 [US1] Call `tool_panel.tick()` from the main tick handler in `tui/src/app/event_loop.rs`
- [x] T029 [P] [US1] Add unit tests for `ToolPanel` lifecycle: start → end → tick prune, visibility, height capping at 10, spinner frame advancement in `tui/src/ui/tool_panel.rs`
- [x] T030 [P] [US1] Add unit tests for `ToolPanel` approval flow: set_awaiting → resolve → tick prune (2s) in `tui/src/ui/tool_panel.rs`

**Checkpoint**: Tool panel shows active/completed tools with spinners/badges and auto-hides. Approvals display with redacted arguments.

---

## Phase 4: User Story 2 — Review File Changes as Inline Diffs (Priority: P1)

**Goal**: Tool results that modify files display inline unified diffs with green additions, red removals, and dimmed context lines. Large diffs are truncated.

**Independent Test**: Have agent modify a file, verify diff displays additions in green and removals in red with correct content. New files show all-addition diffs.

### Implementation for User Story 2

- [x] T031 [US2] Implement LCS dynamic programming algorithm (`compute_lcs`) for line-level diff computation in `tui/src/ui/diff.rs`
- [x] T032 [US2] Implement `render_diff_lines(diff: &DiffData, max_width: u16) -> Vec<Line<'static>>` — header line with path, addition lines (green +), removal lines (red −), context lines (dimmed) in `tui/src/ui/diff.rs`
- [x] T033 [US2] Implement diff truncation at `MAX_DIFF_LINES` (50) with summary of omitted lines in `tui/src/ui/diff.rs`
- [x] T034 [US2] Handle new file case in `render_diff_lines` — all lines as additions when `is_new_file` is true in `tui/src/ui/diff.rs`
- [x] T035 [US2] Implement `truncate_line` helper for capping long lines to terminal width in `tui/src/ui/diff.rs`
- [x] T036 [US2] Extract `DiffData` from tool result details JSON in `tui/src/app/agent_bridge.rs` — parse WriteFileTool details, store in `DisplayMessage.diff_data`
- [x] T037 [US2] Integrate diff rendering in `tui/src/ui/conversation.rs` — when `DisplayMessage.diff_data.is_some()` and block is expanded, call `render_diff_lines` and append to message output
- [x] T038 [P] [US2] Add unit tests for LCS computation: identical content, completely different content, single insertion, single deletion, mixed changes in `tui/src/ui/diff.rs`
- [x] T039 [P] [US2] Add unit tests for `render_diff_lines`: new file (all additions), modification (green/red/dim), truncation at 50 lines, empty content in `tui/src/ui/diff.rs`
- [x] T040 [P] [US2] Add unit test for `DiffData::from_details` round-trip: construct JSON with path/is_new_file/old_content/new_content, parse, verify fields in `tui/src/ui/diff.rs`

**Checkpoint**: File modifications display as color-coded inline diffs. New files show as all-addition diffs. Large diffs are truncated with count of omitted lines.

---

## Phase 5: User Story 3 — Track Resource Consumption via Status Bar (Priority: P1)

**Goal**: Persistent status bar displays model name, token usage (K/M notation), cost, agent state (IDLE/RUNNING/ERROR/ABORTED), retry indicator, and elapsed session time.

**Independent Test**: Run an agent interaction, verify status bar shows correct model, token count, cost, and state transitions.

### Implementation for User Story 3

- [x] T041 [US3] Implement `status_bar::render(frame: &mut Frame, app: &App, area: Rect)` in `tui/src/ui/status_bar.rs` with all segments: state badge (colored bg), optional PLAN badge, optional color mode badge (MONO-W/MONO-B), model name (dimmed), token usage (↓input ↑output using `format_tokens`), cost ($x.xxxx), elapsed time (dimmed, using `format_elapsed`), retry indicator
- [x] T042 [US3] Allocate 1-line status bar area at bottom of layout in `tui/src/ui/mod.rs`
- [x] T043 [US3] Update agent bridge to accumulate `total_input_tokens`, `total_output_tokens`, `total_cost` from `AgentEvent::TurnEnd` usage data in `tui/src/app/agent_bridge.rs`
- [x] T044 [US3] Set `app.status` transitions: Idle on init, Running on `AgentEvent::TurnStart`, Idle on `AgentEvent::TurnEnd`, Error on `AgentEvent::Error`, Aborted on user cancel (Ctrl+C / Esc) in `tui/src/app/agent_bridge.rs` and `tui/src/app/event_loop.rs`
- [x] T045 [US3] Set `app.retry_attempt` from `AgentEvent::RetryAttempt` events and clear on successful `AgentEvent::TurnEnd` in `tui/src/app/agent_bridge.rs`
- [x] T046 [P] [US3] Add unit test verifying status bar renders all segments with correct colors for each `AgentStatus` variant in `tui/src/ui/status_bar.rs` or `tui/src/app/tests.rs`

**Checkpoint**: Status bar shows all resource and state information, updates in real time during agent interactions.

---

## Phase 6: User Story 4 — View Context Window Utilization (Priority: P2)

**Goal**: 10-character context gauge in status bar with green/yellow/red color thresholds at 60%/85%.

**Independent Test**: Run conversation that fills context, verify gauge color transitions from green to yellow to red at correct thresholds.

### Implementation for User Story 4

- [x] T047 [US4] Add context gauge rendering to status bar — call `format_context_gauge`, apply green/yellow/red color based on percentage thresholds (<60%, 60-85%, >85%), hide when `context_budget == 0` in `tui/src/ui/status_bar.rs`
- [x] T048 [US4] Update agent bridge to set `app.context_budget` from `agent.state().model.context_window` and `app.context_tokens_used` from `AgentEvent::ContextUpdated` events in `tui/src/app/agent_bridge.rs`
- [x] T049 [P] [US4] Add integration test verifying context gauge threshold math (green at 59%, yellow at 60%, yellow at 84%, red at 85%) in `tui/tests/ac_tui.rs`

**Checkpoint**: Context gauge appears in status bar with correct color transitions. Hidden when budget is 0.

---

## Phase 7: User Story 5 — Expand and Collapse Tool Result Blocks (Priority: P2)

**Goal**: Tool result blocks auto-collapse after 10s, F2 toggles collapse, Shift+Left/Right cycles selection, user-expanded blocks resist auto-collapse.

**Independent Test**: Trigger multiple tool calls, wait for auto-collapse, press F2 to expand, verify it stays expanded.

### Implementation for User Story 5

- [x] T050 [US5] Implement auto-collapse logic in `App::tick()` — for each `DisplayMessage` with `role == ToolResult`, if `expanded_at` is set and elapsed > 10s and `user_expanded` is false, set `collapsed = true` in `tui/src/app/lifecycle.rs`
- [x] T051 [US5] Implement `App::toggle_collapse(index: usize)` — toggles `collapsed`, sets `user_expanded` on expand, resets `expanded_at` in `tui/src/app/lifecycle.rs`
- [x] T052 [US5] Implement `App::select_prev_tool_block()` and `App::select_next_tool_block()` — cycle `selected_tool_block` through messages with `role == ToolResult` in `tui/src/app/lifecycle.rs`
- [x] T053 [US5] Add F2 key handler in `tui/src/app/event_loop.rs` — if `selected_tool_block` is set, toggle that block; else find most recent ToolResult, select it, and toggle
- [x] T054 [US5] Add Shift+Left and Shift+Right handlers in `tui/src/app/event_loop.rs` — call `select_prev_tool_block()` and `select_next_tool_block()` respectively
- [x] T055 [US5] Generate one-line `summary` (first line of content, max 60 chars) when creating `DisplayMessage` for tool results in `tui/src/app/agent_bridge.rs`
- [x] T056 [US5] Render collapsed tool result blocks as single-line summary with `[+]` indicator and selected highlight; expanded blocks with `[-]` indicator and `[F2]` hint in `tui/src/ui/conversation.rs`
- [x] T057 [US5] Document F2 as "Collapse tool" in help panel key list in `tui/src/ui/help_panel.rs`
- [x] T058 [P] [US5] Add unit tests for auto-collapse timing, user-expanded prevention, toggle behavior in `tui/src/app/tests.rs`
- [x] T059 [P] [US5] Add unit tests for `select_next_tool_block` and `select_prev_tool_block` navigation in `tui/src/app/tests.rs`
- [x] T060 [P] [US5] Add unit test for F2 key event toggling most recent and selected tool blocks in `tui/src/app/tests.rs`
- [x] T061 [P] [US5] Add unit test for Shift+Left/Right cycling from input focus in `tui/src/app/tests.rs`

**Checkpoint**: Tool blocks auto-collapse after 10s, F2 toggles selected block, Shift+arrows cycle selection. User-expanded blocks persist.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Integration, edge cases, and cross-story validation

- [x] T062 [P] Add integration test verifying `DisplayMessage.diff_data` defaults to `None` and round-trips through `DiffData::from_details` in `tui/tests/ac_tui.rs`
- [x] T063 [P] Add integration test verifying context gauge fields default to zero on `App` in `tui/tests/ac_tui.rs`
- [x] T064 Verify `cargo test -p swink-agent-tui` passes all unit and integration tests
- [x] T065 Verify `cargo clippy -p swink-agent-tui -- -D warnings` produces zero warnings
- [x] T066 Run `cargo build --workspace` to verify no cross-crate breakage
- [x] T067 Run quickstart.md validation — verify documented build/test commands work

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Foundational phase; no dependency on other stories
- **User Story 2 (Phase 4)**: Depends on Foundational phase; no dependency on other stories
- **User Story 3 (Phase 5)**: Depends on Foundational phase (format helpers from Setup); no dependency on other stories
- **User Story 4 (Phase 6)**: Depends on US3 (status bar must exist for gauge integration)
- **User Story 5 (Phase 7)**: Depends on Foundational phase; no dependency on other stories
- **Polish (Phase 8)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (P1)**: Independent — tool panel is a self-contained component
- **US2 (P1)**: Independent — diff rendering is self-contained, conversation integration can use stubs
- **US3 (P1)**: Independent — status bar is a self-contained component
- **US4 (P2)**: Depends on US3 — gauge is a segment within the status bar
- **US5 (P2)**: Independent — collapse behavior operates on existing `DisplayMessage` fields

### Within Each User Story

- Data types and logic before rendering
- Rendering before event wiring
- Event wiring before tests (tests validate end-to-end flow)

### Parallel Opportunities

- T001–T007 (all Setup tasks) can run in parallel
- T011–T017 (all Foundational tasks) can run in parallel
- After Phase 2, US1 (T018–T030), US2 (T031–T040), US3 (T041–T046), US5 (T050–T061) can run in parallel
- Within each story, tasks marked [P] can run in parallel (typically unit tests)

---

## Parallel Example: User Story 1

```bash
# After T018–T028 are complete, launch tests in parallel:
Task: "Unit tests for ToolPanel lifecycle" (T029)
Task: "Unit tests for ToolPanel approval flow" (T030)
```

## Parallel Example: User Story 2

```bash
# After T031–T037 are complete, launch tests in parallel:
Task: "Unit tests for LCS computation" (T038)
Task: "Unit tests for render_diff_lines" (T039)
Task: "Unit test for DiffData round-trip" (T040)
```

---

## Implementation Strategy

### MVP First (User Stories 1 + 3 Only)

1. Complete Phase 1: Setup (theme + format helpers)
2. Complete Phase 2: Foundational (types)
3. Complete Phase 3: US1 — Tool Panel
4. Complete Phase 5: US3 — Status Bar
5. **STOP and VALIDATE**: Tool panel shows tool activity, status bar shows resources
6. The TUI is usable for basic agent monitoring

### Incremental Delivery

1. Setup + Foundational → Foundation ready
2. Add US1 (Tool Panel) → Test independently → MVP monitoring
3. Add US3 (Status Bar) → Test independently → Full resource tracking
4. Add US2 (Diffs) → Test independently → File change review
5. Add US4 (Context Gauge) → Extends status bar with usage visibility
6. Add US5 (Collapse) → Test independently → Clean conversation management
7. Polish → Integration tests, clippy, cross-crate build verification

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- All source files live in the `tui/` crate under `tui/src/`
- Several files already exist from specs 025/026/028/029 and will be extended, not created from scratch
