# Quickstart: TUI Plan Mode & Approval

**Feature**: 029-tui-plan-mode-approval

## What Changes

This feature modifies existing plan mode and approval infrastructure to add:
1. An "Approve plan?" prompt when exiting plan mode (instead of direct toggle)
2. Auto-sending the plan as the next user message on approval
3. A trust follow-up prompt after tool approvals in Smart mode
4. `#approve untrust` command for revoking session trust
5. Default approval mode changed from Enabled to Smart

## Files Modified

### Core (`src/`)
- `src/tool.rs` — Move `#[default]` from `Enabled` to `Smart` on `ApprovalMode`

### TUI (`tui/src/`)
- `tui/src/app/state.rs` — Add `TrustFollowUp` struct, `trust_follow_up` and `pending_plan_approval` fields
- `tui/src/app/event_loop.rs` — Add key handling for plan approval and trust follow-up states; add streaming guard on plan toggle
- `tui/src/app/agent_bridge.rs` — Change `toggle_operating_mode()` to show plan approval prompt instead of direct exit; add `approve_plan()` and `reject_plan()` methods; add plan message concatenation
- `tui/src/app/lifecycle.rs` — Add `TrustFollowUp` timeout check in `tick()`; initialize new fields
- `tui/src/commands.rs` — Add `#approve untrust` variants and `CommandResult` variants
- `tui/src/ui/tool_panel.rs` — Render plan approval prompt and trust follow-up prompt
- `tui/src/app/tests.rs` — New tests for all changed behavior

## Build & Verify

```bash
cargo test --workspace                          # All tests pass
cargo clippy --workspace -- -D warnings         # Zero warnings
cargo run -p swink-agent-tui                    # Manual smoke test
```

## Key Design Choices

- **Plan approval uses a bool flag**, not a synthetic `ToolApprovalRequest` — different response flow
- **Trust follow-up is additive** — the "A" key shortcut for instant trust still works
- **Plan messages concatenated with `---` separators** — preserves multi-turn plan structure
- **Streaming guard is silent** — no error message when toggle is ignored during streaming
