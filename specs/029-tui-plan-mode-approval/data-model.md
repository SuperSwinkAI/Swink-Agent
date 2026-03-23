# Data Model: TUI Plan Mode & Approval

**Feature**: 029-tui-plan-mode-approval
**Date**: 2026-03-22

## Entities

### Existing (no changes needed)

| Entity | Location | Fields |
|---|---|---|
| `ApprovalMode` | `src/tool.rs:239` | Enabled, Smart, Bypassed |
| `ToolApproval` | `src/tool.rs:200` | Approved, Rejected, ApprovedWith(Value) |
| `ToolApprovalRequest` | `src/tool.rs:213` | tool_call_id, tool_name, arguments, requires_approval |
| `OperatingMode` | `tui/src/app/state.rs` | Execute, Plan |
| `PendingApproval` | `tui/src/ui/tool_panel.rs` | tool_name, args_summary, created_at |
| `ResolvedApproval` | `tui/src/ui/tool_panel.rs` | tool_name, approved, resolved_at |
| `ToolExecution` | `tui/src/ui/tool_panel.rs` | id, name, started_at, completed_at, is_error |

### Modified

#### `ApprovalMode` — Default Change

```rust
// src/tool.rs — change #[default] from Enabled to Smart
pub enum ApprovalMode {
    Enabled,
    #[default]  // ← move here
    Smart,
    Bypassed,
}
```

**Rationale**: FR-002 — Smart must be the default.

### New

#### `TrustFollowUp`

```rust
// tui/src/app/state.rs
pub(crate) struct TrustFollowUp {
    pub tool_name: String,
    pub expires_at: Instant,
}
```

**Purpose**: Tracks the inline "Always approve this tool? y/n" prompt after a tool approval in Smart mode. Auto-dismissed when `Instant::now() > expires_at` (3 seconds).

**Lifecycle**:
1. Created when user approves a tool via `y`/`Y`/`Enter` in Smart mode
2. User accepts (`y`) → tool added to `session_trusted_tools`, follow-up cleared
3. User declines (`n`) → follow-up cleared, no trust change
4. Timeout (tick) → follow-up cleared, no trust change

#### `PendingPlanApproval` (bool flag on App)

No new struct — just a `pending_plan_approval: bool` field on `App`. When `true`:
- Tool panel renders "Approve plan? [Y/n]" in the pending approvals area
- Key handling intercepts Y/n like tool approvals
- On approve: `exit_plan_mode()` + concatenate plan messages + `send_to_agent()`
- On reject: clear flag, remain in plan mode

## State Transitions

### Plan Mode Lifecycle

```
Execute ──[Shift+Tab / /plan]──→ Plan
  │                                │
  │                           [Shift+Tab / /plan]
  │                                │
  │                                ▼
  │                        PendingPlanApproval
  │                           ╱          ╲
  │                     [Y/Enter]       [N/Esc]
  │                         │              │
  │                         ▼              ▼
  │                    Exit Plan       Stay in Plan
  │                  + Send Plan       (clear prompt)
  │                         │
  ◄─────────────────────────┘
```

### Tool Approval Lifecycle (Smart Mode)

```
ToolCall ──[requires_approval?]──→ No → Auto-execute
                │
                Yes
                │
         [In session_trusted_tools?]──→ Yes → Auto-execute
                │
                No
                │
                ▼
        PendingApproval (tool panel)
           ╱      │      ╲
      [Y/Enter]  [A]    [N/Esc]
          │       │        │
          ▼       ▼        ▼
       Approve  Approve  Reject
          │    + Trust     │
          ▼       │        ▼
    TrustFollowUp │   Error result
    (3s timeout)  │   → agent
       ╱    ╲     │
     [y]    [n/∅] │
      │      │    │
      ▼      ▼    ▼
    Trust  No-op  Trusted
```

## Field Changes on App struct

```rust
// New fields to add to App:
pub(crate) trust_follow_up: Option<TrustFollowUp>,
pub(crate) pending_plan_approval: bool,
```

## Command Changes

### `#approve untrust` variants

| Command | Action |
|---|---|
| `#approve untrust <name>` | Remove `name` from `session_trusted_tools` |
| `#approve untrust` | Clear all `session_trusted_tools` |

New `CommandResult` variants:
- `UntrustTool(String)` — revoke specific tool
- `UntrustAll` — revoke all
