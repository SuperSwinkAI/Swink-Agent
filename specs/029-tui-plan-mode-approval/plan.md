# Implementation Plan: TUI Plan Mode & Approval

**Branch**: `029-tui-plan-mode-approval` | **Date**: 2026-03-22 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/029-tui-plan-mode-approval/spec.md`

## Summary

Add an approval-gated plan mode exit and a trust follow-up prompt to the TUI. Most infrastructure (ApprovalMode, plan mode toggle, session trust, tool panel) already exists. The implementation closes five gaps: (1) plan exit shows "Approve plan?" prompt instead of direct toggle, (2) approved plans are auto-sent as the next user message, (3) post-approval trust follow-up with 3-second auto-dismiss, (4) `#approve untrust` command, (5) default approval mode changed to Smart. One core change (ApprovalMode default), remainder is TUI-only.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `swink-agent` (core), `ratatui` 0.30, `crossterm` 0.29, `tokio`, `tokio-util`
**Storage**: N/A (all state is in-memory per session)
**Testing**: `cargo test --workspace` + `cargo clippy --workspace -- -D warnings`
**Target Platform**: macOS, Linux, Windows (terminal)
**Project Type**: Library (core) + TUI binary
**Performance Goals**: Mode switching within one render frame (<16ms)
**Constraints**: No unsafe code, zero clippy warnings
**Scale/Scope**: ~8 files modified, ~200-300 lines added

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|---|---|---|
| I. Library-First | PASS | One core change (ApprovalMode default). TUI-only for the rest. No new crates. |
| II. Test-Driven Development | PASS | Tests written before implementation for each gap. Existing tests updated for default change. |
| III. Efficiency & Performance | PASS | No hot-path changes. Trust follow-up timer uses existing tick loop. |
| IV. Leverage the Ecosystem | PASS | No new dependencies. Uses existing ratatui/crossterm. |
| V. Provider Agnosticism | PASS | No provider-specific code. |
| VI. Safety & Correctness | PASS | No unsafe. All new state is `Option`-wrapped with clear lifecycle. |
| Crate count | PASS | No new crates (7 workspace members unchanged). |
| MSRV | PASS | 1.88, edition 2024. |
| Concurrency | PASS | No new concurrency. Plan approval is synchronous UI state. |
| Events outward-only | PASS | No new event types. |
| No global mutable state | PASS | All state on `App` struct. |

**Post-Phase 1 re-check**: PASS — no violations introduced by design artifacts.

## Project Structure

### Documentation (this feature)

```text
specs/029-tui-plan-mode-approval/
├── plan.md              # This file
├── research.md          # Gap analysis and design decisions
├── data-model.md        # Entity changes and state transitions
├── quickstart.md        # Build and verify guide
└── tasks.md             # Phase 2 output (via /speckit.tasks)
```

### Source Code (affected files)

```text
src/
└── tool.rs                          # ApprovalMode default change

tui/src/
├── app/
│   ├── state.rs                     # TrustFollowUp struct, new App fields
│   ├── event_loop.rs                # Key handling for plan approval + trust follow-up
│   ├── agent_bridge.rs              # Plan approval flow, plan message concatenation
│   ├── lifecycle.rs                 # Trust follow-up timeout, new field init
│   └── tests.rs                     # New tests
├── commands.rs                      # #approve untrust command
└── ui/
    └── tool_panel.rs                # Render plan approval + trust follow-up prompts
```

**Structure Decision**: No structural changes. All modifications are within existing files in the `swink-agent` core crate and `swink-agent-tui` crate.

## Complexity Tracking

No constitution violations — table not applicable.
