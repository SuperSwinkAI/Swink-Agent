# Tasks: TUI: Plan Mode & Approval

**Input**: Design documents from `/specs/029-tui-plan-mode-approval/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, quickstart.md

**Tests**: Tests are included per user story. Unit tests in the respective source modules; integration tests in `tui/src/app/tests.rs`.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

**Important notes**:
- Most infrastructure already exists (ApprovalMode, plan mode toggle, session trust, tool panel). Tasks close five specific gaps identified in the research.
- The `TrustFollowUp` struct is additive — the existing "A" key shortcut for instant trust continues to work alongside the follow-up prompt.
- Plan approval uses a dedicated `pending_plan_approval: bool` flag, not a synthetic `ToolApprovalRequest`.
- Plan messages are concatenated with `\n\n---\n\n` separators to preserve multi-turn plan structure.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3, US4)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Dependencies & Verification)

**Purpose**: Verify existing infrastructure is in place and no new dependencies are needed

- [x] T001 [P] Verify `swink-agent` (core) path dependency is listed in `tui/Cargo.toml` for `ApprovalMode`, `ToolApproval`, `ToolApprovalRequest` types
- [x] T002 [P] Verify `tokio` has `time` feature enabled in `tui/Cargo.toml` for `Instant` usage in `TrustFollowUp`
- [x] T003 [P] Verify `crossterm` has `event-stream` feature enabled in `tui/Cargo.toml` for async terminal event handling

---

## Phase 2: Foundation (Core Type Changes)

**Purpose**: Change the `ApprovalMode` default and add new TUI state types that all user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

### Core Default Change

- [x] T004 Move `#[default]` attribute from `Enabled` to `Smart` on `ApprovalMode` enum in `src/tool.rs` (line ~241→245). This fulfills FR-002.
- [x] T005 Update existing test `approval_mode_default_is_enabled` in `src/tool.rs` (line ~570) to `approval_mode_default_is_smart` — assert `ApprovalMode::default() == ApprovalMode::Smart`

### New TUI State Types

- [x] T006 Define `TrustFollowUp` struct in `tui/src/app/state.rs` with fields: `tool_name: String`, `expires_at: Instant` (use `std::time::Instant`). Derive `Debug`.
- [x] T007 Add `trust_follow_up: Option<TrustFollowUp>` field to `App` struct in `tui/src/app/state.rs` (after `session_trusted_tools`)
- [x] T008 Add `pending_plan_approval: bool` field to `App` struct in `tui/src/app/state.rs` (after `operating_mode`)
- [x] T009 Initialize `trust_follow_up: None` and `pending_plan_approval: false` in `App::new()` in `tui/src/app/lifecycle.rs`

**Checkpoint**: Core types defined — user story implementation can now begin

---

## Phase 3: User Story 1 — Control Which Tools Require Approval (Priority: P1)

**Goal**: Developer controls oversight via three approval modes (Enabled/Smart/Bypassed). Smart is default. Mode switching via `#approve` command.

**Independent Test**: Trigger a write tool in Smart mode → approval prompt appears. Switch to Bypassed → same tool auto-executes. Switch to Enabled → all tools prompt.

### Tests for User Story 1

> **NOTE: Write tests FIRST, ensure they FAIL before implementation.**

- [x] T010 [P] [US1] Test `approval_mode_default_is_smart` in `tui/src/app/tests.rs`: construct `App`, verify `app.approval_mode == ApprovalMode::Smart` (FR-001, FR-002)
- [x] T011 [P] [US1] Test `smart_mode_auto_approves_readonly_tool` in `tui/src/app/tests.rs`: set Smart mode, simulate tool call with `requires_approval() == false`, verify no pending approval prompt appears (FR-001)
- [x] T012 [P] [US1] Test `smart_mode_prompts_for_write_tool` in `tui/src/app/tests.rs`: set Smart mode, simulate tool call with `requires_approval() == true`, verify `pending_approval` is `Some` (FR-001, FR-004)
- [x] T013 [P] [US1] Test `enabled_mode_prompts_for_all_tools` in `tui/src/app/tests.rs`: set Enabled mode, simulate read-only tool call, verify approval prompt appears (FR-001)
- [x] T014 [P] [US1] Test `bypassed_mode_auto_approves_all` in `tui/src/app/tests.rs`: set Bypassed mode, simulate write tool call, verify no approval prompt (FR-001)
- [x] T015 [P] [US1] Test `approve_command_switches_modes` in `tui/src/commands.rs` tests: `execute_command("#approve on")` → `SetApprovalMode(On)`, `"#approve smart"` → `SetApprovalMode(Smart)`, `"#approve off"` → `SetApprovalMode(Off)` (FR-003)

### Implementation for User Story 1

- [x] T016 [US1] Verify existing approval key handling in `tui/src/app/event_loop.rs` (lines 165-193): `y`/`Y`/`Enter` → Approve, `n`/`N`/`Esc` → Reject, `a`/`A` → Approve + Trust. Confirm behavior matches FR-004.
- [x] T017 [US1] Verify existing `#approve on|off|smart` command handling in `tui/src/commands.rs` (lines 115-121) and event loop wiring in `tui/src/app/event_loop.rs`. Confirm behavior matches FR-003.
- [x] T018 [US1] Verify rejected tool calls produce an error `AgentToolResult` with `is_error: true` sent back to the agent, per FR-013. Check `tui/src/app/event_loop.rs` rejection handling.

**Checkpoint**: US1 complete — approval modes work correctly with Smart as default

---

## Phase 4: User Story 2 — Grant Per-Tool Session Trust (Priority: P1)

**Goal**: After approving a write tool in Smart mode, developer gets a 3-second inline "Always approve this tool?" follow-up. Accepting adds tool to session trust. Trust is per-session only.

**Independent Test**: Approve a tool with "y" → follow-up appears → accept "y" → trigger same tool again → no prompt. Restart TUI → tool prompts again.

### Tests for User Story 2

> **NOTE: Write tests FIRST, ensure they FAIL before implementation.**

- [x] T019 [P] [US2] Test `trust_follow_up_triggers_after_approval_in_smart_mode` in `tui/src/app/tests.rs`: approve a tool via `y` in Smart mode, verify `trust_follow_up` is `Some` with correct `tool_name` (FR-006)
- [x] T020 [P] [US2] Test `trust_follow_up_not_triggered_in_enabled_mode` in `tui/src/app/tests.rs`: approve a tool via `y` in Enabled mode, verify `trust_follow_up` is `None` (FR-006)
- [x] T021 [P] [US2] Test `trust_follow_up_not_triggered_in_bypassed_mode` in `tui/src/app/tests.rs`: Bypassed mode auto-approves, verify `trust_follow_up` is `None` (FR-006)
- [x] T022 [P] [US2] Test `trust_follow_up_y_adds_to_session_trusted` in `tui/src/app/tests.rs`: set `trust_follow_up` for "bash", press `y`, verify "bash" in `session_trusted_tools` and `trust_follow_up` is `None` (FR-006)
- [x] T023 [P] [US2] Test `trust_follow_up_n_does_not_trust` in `tui/src/app/tests.rs`: set `trust_follow_up` for "bash", press `n`, verify "bash" NOT in `session_trusted_tools` and `trust_follow_up` is `None` (FR-006)
- [x] T024 [P] [US2] Test `trust_follow_up_timeout_clears` in `tui/src/app/tests.rs`: set `trust_follow_up` with expired `expires_at`, call `tick()`, verify `trust_follow_up` is `None` (FR-006: 3-second auto-dismiss)
- [x] T025 [P] [US2] Test `trusted_tool_auto_approves_in_smart_mode` in `tui/src/app/tests.rs`: add tool to `session_trusted_tools`, trigger tool call in Smart mode, verify auto-approved (no pending approval) (FR-006)
- [x] T026 [P] [US2] Test `trusted_tool_still_prompts_in_enabled_mode` in `tui/src/app/tests.rs`: add tool to `session_trusted_tools`, switch to Enabled mode, trigger tool call, verify approval prompt appears (trust only applies in Smart mode) (FR-006)
- [x] T027 [P] [US2] Test `session_trust_not_persisted` in `tui/src/app/tests.rs`: construct new `App`, verify `session_trusted_tools` is empty (FR-007)

### Implementation for User Story 2

- [x] T028 [US2] In `tui/src/app/event_loop.rs`, modify approval key handling (`y`/`Y`/`Enter` branch, lines 165-175): after sending `ToolApproval::Approved` via oneshot channel, if `self.approval_mode == ApprovalMode::Smart`, set `self.trust_follow_up = Some(TrustFollowUp { tool_name: request.tool_name.clone(), expires_at: Instant::now() + Duration::from_secs(3) })`
- [x] T029 [US2] In `tui/src/app/event_loop.rs`, add trust follow-up key handling BEFORE the normal input path: when `self.trust_follow_up.is_some()`, intercept `y`/`Y`/`Enter` (add tool to `session_trusted_tools`, clear follow-up), `n`/`N`/`Esc` (clear follow-up), any other key (clear follow-up, re-process key)
- [x] T030 [US2] In `tui/src/app/lifecycle.rs` `tick()` function, add trust follow-up timeout check: if `trust_follow_up.expires_at < Instant::now()`, clear `trust_follow_up` and set `dirty = true`
- [x] T031 [US2] In `tui/src/ui/tool_panel.rs`, render trust follow-up prompt inline after resolved approvals: when `app.trust_follow_up.is_some()`, display "Always approve [tool_name]? y/n" with yellow accent styling

**Checkpoint**: US2 complete — trust follow-up works with 3-second auto-dismiss

---

## Phase 5: User Story 3 — Plan Before Executing with Plan Mode (Priority: P1)

**Goal**: Developer enters plan mode → agent restricted to read-only tools → developer approves plan → plan auto-sent as next user message in execute mode.

**Independent Test**: Toggle plan mode → verify write tools removed. Ask agent to edit a file → only read tools used. Approve plan → verify plan messages sent as user message.

### Tests for User Story 3

> **NOTE: Write tests FIRST, ensure they FAIL before implementation.**

- [x] T032 [P] [US3] Test `plan_toggle_enters_plan_mode` in `tui/src/app/tests.rs`: call `toggle_operating_mode()` from Execute mode, verify `operating_mode == OperatingMode::Plan` (FR-008)
- [x] T033 [P] [US3] Test `plan_toggle_shows_approval_prompt` in `tui/src/app/tests.rs`: enter plan mode, call `toggle_operating_mode()` again, verify `pending_plan_approval == true` and `operating_mode` is still `Plan` (FR-011)
- [x] T034 [P] [US3] Test `plan_approval_y_exits_plan_and_sends_messages` in `tui/src/app/tests.rs`: enter plan mode, add assistant messages with `plan_mode: true`, set `pending_plan_approval = true`, call `approve_plan()`, verify `operating_mode == Execute`, `pending_plan_approval == false`, and plan messages were sent to agent (FR-011, FR-012)
- [x] T035 [P] [US3] Test `plan_approval_n_stays_in_plan` in `tui/src/app/tests.rs`: enter plan mode, set `pending_plan_approval = true`, call `reject_plan()`, verify `operating_mode == Plan` and `pending_plan_approval == false` (FR-011)
- [x] T036 [P] [US3] Test `plan_approval_empty_plan_skips_send` in `tui/src/app/tests.rs`: enter plan mode with no assistant messages, approve plan, verify no message sent to agent but mode still transitions to Execute (FR-012)
- [x] T037 [P] [US3] Test `plan_toggle_ignored_while_agent_running` in `tui/src/app/tests.rs`: set `status = AgentStatus::Running`, press Shift+Tab, verify `operating_mode` unchanged (FR-015)
- [x] T038 [P] [US3] Test `plan_messages_concatenated_with_separator` in `tui/src/app/tests.rs`: enter plan mode, add 3 assistant messages ("step 1", "step 2", "step 3"), approve plan, verify sent message contains all 3 messages joined by `\n\n---\n\n` (FR-012)
- [x] T039 [P] [US3] Test `plan_mode_only_collects_assistant_messages` in `tui/src/app/tests.rs`: enter plan mode, add user message, assistant message, tool result message (all with `plan_mode: true`), approve plan, verify only assistant message content is included in sent message (FR-012)
- [x] T040 [P] [US3] Test `plan_badge_shown_in_plan_mode` in `tui/src/app/tests.rs`: enter plan mode, verify status bar contains "PLAN" badge (test through App state, not rendering) (FR-010)

### Implementation for User Story 3

- [x] T041 [US3] In `tui/src/app/agent_bridge.rs`, modify `toggle_operating_mode()`: when transitioning Plan→Execute, instead of calling `exit_plan_mode()` directly, set `self.pending_plan_approval = true` and return (leave in Plan mode until approved) (FR-011)
- [x] T042 [US3] In `tui/src/app/agent_bridge.rs`, add `approve_plan(&mut self)` method: clear `pending_plan_approval`, call `exit_plan_mode()`, collect all `DisplayMessage` entries where `plan_mode == true && role == MessageRole::Assistant`, concatenate `content` fields with `\n\n---\n\n`, and if non-empty call `send_to_agent()` with the concatenated plan (FR-009, FR-012)
- [x] T043 [US3] In `tui/src/app/agent_bridge.rs`, add `reject_plan(&mut self)` method: clear `pending_plan_approval`, remain in Plan mode (FR-011)
- [x] T044 [US3] In `tui/src/app/event_loop.rs`, add streaming guard before `toggle_operating_mode()` on Shift+Tab: only call if `self.status != AgentStatus::Running` (FR-015)
- [x] T045 [US3] In `tui/src/app/event_loop.rs`, add plan approval key handling BEFORE normal input when `self.pending_plan_approval`: `y`/`Y`/`Enter` → `approve_plan()`, `n`/`N`/`Esc` → `reject_plan()`. Set `dirty = true`. (FR-011)
- [x] T046 [US3] In `tui/src/ui/tool_panel.rs`, render plan approval prompt when `app.pending_plan_approval == true`: display "⚠ Approve Plan?" with `Approve? [Y/n]` prompt, styled like tool approval (yellow/warning accent). Render above tool approvals. (FR-010, FR-011)

**Checkpoint**: US3 complete — plan mode with approval-gated exit and auto-send

---

## Phase 6: User Story 4 — Classify Tools for Approval Decisions (Priority: P2)

**Goal**: Tools declare `requires_approval()` via trait method. Smart mode uses this for auto-approve/prompt decision.

**Independent Test**: Register tools with different classifications, run Smart mode, verify correct auto-approve/prompt behavior.

### Tests for User Story 4

> **NOTE: Write tests FIRST, ensure they FAIL before implementation.**

- [x] T047 [P] [US4] Test `requires_approval_default_is_false` in `src/tool.rs` tests: create a mock tool using default `requires_approval()`, verify it returns `false`
- [x] T048 [P] [US4] Test `tool_with_requires_approval_true_prompts_in_smart` in `tui/src/app/tests.rs`: register tool with `requires_approval() == true`, trigger in Smart mode, verify approval prompt
- [x] T049 [P] [US4] Test `tool_with_requires_approval_false_auto_approves_in_smart` in `tui/src/app/tests.rs`: register tool with `requires_approval() == false`, trigger in Smart mode, verify auto-approved
- [x] T050 [P] [US4] Test `enabled_mode_ignores_classification` in `tui/src/app/tests.rs`: register read-only tool (`requires_approval() == false`), switch to Enabled mode, trigger tool, verify approval prompt shown regardless
- [x] T051 [P] [US4] Test `bypassed_mode_ignores_classification` in `tui/src/app/tests.rs`: register write tool (`requires_approval() == true`), switch to Bypassed mode, trigger tool, verify auto-approved regardless

### Implementation for User Story 4

- [x] T052 [US4] Verify existing `requires_approval()` trait method on `AgentTool` in `src/tool.rs` (line ~125) returns `false` by default. Confirm Smart mode dispatch in `src/loop_/tool_dispatch.rs` (line ~88-104) checks this method to decide approval. No changes expected — verify existing behavior matches FR-005.

**Checkpoint**: US4 complete — tool classification drives approval decisions

---

## Phase 7: User Story 2 Supplement — `#approve untrust` Command (Priority: P1)

**Goal**: Developer can revoke per-tool session trust via `#approve untrust <tool_name>` (specific) or `#approve untrust` (all).

**Independent Test**: Trust a tool, run `#approve untrust bash`, trigger tool again, verify prompt reappears.

### Tests for User Story 2 Supplement

> **NOTE: Write tests FIRST, ensure they FAIL before implementation.**

- [x] T053 [P] [US2] Test `untrust_specific_tool_command` in `tui/src/commands.rs` tests: `execute_command("#approve untrust bash")` → `UntrustTool("bash")` (FR-014)
- [x] T054 [P] [US2] Test `untrust_all_command` in `tui/src/commands.rs` tests: `execute_command("#approve untrust")` → `UntrustAll` (FR-014)
- [x] T055 [P] [US2] Test `untrust_specific_removes_from_set` in `tui/src/app/tests.rs`: add "bash" and "write_file" to `session_trusted_tools`, handle `UntrustTool("bash")`, verify "bash" removed and "write_file" remains (FR-014)
- [x] T056 [P] [US2] Test `untrust_all_clears_set` in `tui/src/app/tests.rs`: add multiple tools to `session_trusted_tools`, handle `UntrustAll`, verify set is empty (FR-014)

### Implementation for User Story 2 Supplement

- [x] T057 [US2] Add `UntrustTool(String)` and `UntrustAll` variants to `CommandResult` enum in `tui/src/commands.rs`
- [x] T058 [US2] In `tui/src/commands.rs` hash command handler, add `#approve untrust` parsing before existing `#approve` handling: match `"approve untrust"` prefix, extract optional tool name argument. Empty → `UntrustAll`, with arg → `UntrustTool(arg)`.
- [x] T059 [US2] In `tui/src/app/event_loop.rs` `submit_input()`, handle `CommandResult::UntrustTool(name)`: remove from `session_trusted_tools`, add feedback message "Untrusted tool: {name}"
- [x] T060 [US2] In `tui/src/app/event_loop.rs` `submit_input()`, handle `CommandResult::UntrustAll`: clear `session_trusted_tools`, add feedback message "Cleared all trusted tools"

**Checkpoint**: Untrust commands complete — session trust fully revocable

---

## Phase 8: Edge Cases & Integration

**Purpose**: Verify edge case handling per spec

### Tests for Edge Cases

- [x] T061 [P] Test `plan_toggle_during_plan_approval_ignored` in `tui/src/app/tests.rs`: while `pending_plan_approval == true`, press Shift+Tab, verify state unchanged (don't start a new toggle while approval is pending)
- [x] T062 [P] Test `concurrent_plan_and_tool_approval` in `tui/src/app/tests.rs`: verify that `pending_plan_approval` and `pending_approval` (tool) cannot both be active simultaneously — plan approval takes precedence in key handling
- [x] T063 [P] Test `trust_follow_up_cleared_on_new_approval` in `tui/src/app/tests.rs`: while trust follow-up is active, a new tool approval arrives → trust follow-up is cleared in favor of the new approval prompt
- [x] T064 [P] Test `plan_mode_removes_write_tools` in `tui/src/app/tests.rs`: verify `enter_plan_mode()` saves and removes tools with `requires_approval() == true` from agent's tool set

### Implementation for Edge Cases

- [x] T065 In `tui/src/app/event_loop.rs`, add guard: if `pending_plan_approval`, ignore Shift+Tab toggle
- [x] T066 In `tui/src/app/event_loop.rs`, ensure key handling priority order: (1) trust follow-up, (2) plan approval, (3) tool approval, (4) normal input
- [x] T067 In `tui/src/app/event_loop.rs`, when a new `pending_approval` arrives while `trust_follow_up.is_some()`, clear `trust_follow_up`

**Checkpoint**: All edge cases handled — feature is robust

---

## Phase 9: Final Verification

**Purpose**: End-to-end verification and cleanup

- [x] T068 Run `cargo test --workspace` — all tests pass (including updated default test)
- [x] T069 Run `cargo clippy --workspace -- -D warnings` — zero warnings
- [x] T070 Verify `cargo build --workspace` succeeds with no errors
