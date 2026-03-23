# Research: TUI Plan Mode & Approval

**Feature**: 029-tui-plan-mode-approval
**Date**: 2026-03-22

## Gap Analysis: Existing vs Required

The codebase already implements significant infrastructure for this feature. This research documents what exists, what's missing, and design decisions for the gaps.

### What Already Exists

| Capability | Location | Status |
|---|---|---|
| `ApprovalMode` enum (Enabled/Smart/Bypassed) | `src/loop_/mod.rs:239-249` | Complete |
| `requires_approval()` trait method (default false) | `src/tool.rs:125` | Complete |
| `ToolApproval` enum (Approved/Rejected/ApprovedWith) | `src/tool.rs:200-207` | Complete |
| `ToolApprovalRequest` struct | `src/tool.rs:213-234` | Complete |
| Tool dispatch approval gate (Smart mode logic) | `src/loop_/tool_dispatch.rs:88-104` | Complete |
| `approve_tool` callback in `AgentLoopConfig` | `src/loop_/mod.rs:222` | Complete |
| `agent.enter_plan_mode()` / `exit_plan_mode()` | `src/agent.rs:437-464` | Complete |
| TUI `OperatingMode` enum (Execute/Plan) | `tui/src/app/state.rs` | Complete |
| `toggle_operating_mode()` with enter/exit | `tui/src/app/agent_bridge.rs:245-279` | Complete |
| `session_trusted_tools: HashSet<String>` | `tui/src/app/state.rs` | Complete |
| `pending_approval` slot with oneshot responder | `tui/src/app/state.rs` | Complete |
| Tool panel PendingApproval/ResolvedApproval | `tui/src/ui/tool_panel.rs` | Complete |
| Approval key handling (y/n/a) | `tui/src/app/event_loop.rs:165-193` | Complete |
| Smart auto-approve for trusted tools | `tui/src/app/agent_bridge.rs` | Complete |
| `#approve on\|off\|smart` commands | `tui/src/commands.rs:115-121` | Complete |
| `/plan` command | `tui/src/commands.rs:150` | Complete |
| Shift+Tab plan toggle | `tui/src/app/event_loop.rs:222-223` | Complete |
| PLAN badge in status bar | `tui/src/ui/status_bar.rs` | Complete |
| Plan-mode message labeling ("Plan" in blue) | `tui/src/ui/conversation.rs` | Complete |
| `saved_tools` / `saved_system_prompt` on App | `tui/src/app/state.rs` | Complete |
| `plan_mode` flag on DisplayMessage | `tui/src/app/state.rs` | Complete |

### What Needs to Be Built

| Gap | Spec Requirement | Complexity |
|---|---|---|
| **Plan approval prompt** | FR-011: Exit plan mode shows "Approve plan?" prompt styled like tool approval | Medium — new `pending_plan_approval: bool` state, reuse tool approval UI pattern |
| **Plan auto-send on approve** | FR-012: On approval, concatenate plan-mode assistant messages → send as user message | Low — collect messages, call `send_to_agent()` |
| **Plan rejection keeps plan mode** | FR-011: On rejection, remain in plan mode | Low — just don't call `exit_plan_mode()` |
| **Trust follow-up with auto-dismiss** | FR-006: After tool approval in Smart mode, show "Always approve? y/n" for 3 seconds | Medium — new `TrustFollowUp` state with timer |
| **`#approve untrust` command** | FR-014: Revoke per-tool trust or clear all | Low — add command variants, remove from HashSet |
| **Streaming guard on plan toggle** | FR-015: Ignore plan toggle while agent is running | Low — check `self.status` before toggling |
| **Default to Smart mode** | FR-002: Smart must be the default | Low — change `ApprovalMode::default()` if needed |
| **Empty plan guard** | Edge case: No assistant messages in plan → skip send | Low — guard before concatenation |

## Design Decisions

### Decision 1: Plan Approval Prompt Implementation

**Decision**: Use a new `pending_plan_approval: bool` flag on App state, rendered in the tool panel alongside tool approvals.

**Rationale**: The spec says "styled like a tool approval prompt." Reusing the same tool panel area and key handling pattern (Y/n) keeps the UX consistent and minimizes new code. However, a plan approval is semantically different from a tool approval (no `ToolApprovalRequest`, no oneshot channel), so a dedicated flag is cleaner than overloading `pending_approval`.

**Alternatives considered**:
- Overloading `pending_approval` with a synthetic ToolApprovalRequest → Rejected because the response flows differently (plan approval triggers `exit_plan_mode()` + `send_to_agent()`, not a oneshot channel)
- Using the same `PendingApproval` struct in tool panel → Rejected because plan approval has no tool name or arguments to display

### Decision 2: Trust Follow-Up with Auto-Dismiss

**Decision**: After resolving a tool approval as "Approved" in Smart mode, set a `trust_follow_up: Option<TrustFollowUp>` containing the tool name and an `Instant` deadline (now + 3 seconds). The tool panel renders this as an inline prompt. Key handling checks for this state before the normal input path. On timeout (checked in tick), it's cleared.

**Rationale**: The current "A" key for "Approve + Always Trust" is a shortcut that already works. The spec adds an explicit follow-up prompt after a plain approval. This is an additive UI element that doesn't disrupt the existing fast path.

**Alternatives considered**:
- Removing the "A" shortcut in favor of the follow-up only → Rejected because the "A" key is a faster workflow for power users
- Modal dialog → Rejected, would break the terminal UI flow

### Decision 3: Plan Message Concatenation

**Decision**: On plan approval, iterate `self.messages` in order, collect all `DisplayMessage` entries where `plan_mode == true && role == MessageRole::Assistant`, join their `content` fields with `\n\n---\n\n`, and pass to `send_to_agent()`.

**Rationale**: The `plan_mode` flag is already set on messages during plan mode. Simple concatenation with separators preserves the plan structure. The separator makes multi-turn plans readable.

**Alternatives considered**:
- Including tool results in the plan → Rejected, spec says "assistant messages"
- Structured format with metadata → Rejected, over-engineering for this use case

### Decision 4: Streaming Guard

**Decision**: Check `self.status != AgentStatus::Running` before processing `toggle_operating_mode()`. If running, ignore silently.

**Rationale**: Simple, matches spec FR-015. No need for a user-visible message — the plan badge state already communicates the current mode.

### Decision 5: Default Approval Mode

**Decision**: Verify `ApprovalMode::default()` returns `Smart`. If not, change it.

**Rationale**: Spec FR-002. The current implementation uses `ApprovalMode::default()` which may return `Enabled`. Need to verify.
