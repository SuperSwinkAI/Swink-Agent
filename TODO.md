# TODO — TUI Features

Planned features for the Swink Agent TUI, grouped by implementation priority. Each feature has a corresponding PRD section and architecture doc entry.

---

## Priority 1 — TUI-Only Changes

No core crate modifications required. Can be implemented independently.

- [x] **Context Window Progress Bar** — Visual gauge in the status bar showing context fill % with green/yellow/red color transitions. ([PRD §16.7](docs/planning/PRD.md#167-context-window-progress-bar))
- [x] **Collapsible Tool Result Blocks** — Collapse tool invocations to a one-line summary. Expand on Enter/click. Auto-collapse after 3s. ([PRD §16.10](docs/planning/PRD.md#1610-collapsible-tool-result-blocks))
- [x] **External Editor Mode** — Open `$EDITOR` for multi-line prompt composition. Suspend TUI, submit on close. `Ctrl+E` or `/editor`. ([PRD §16.8](docs/planning/PRD.md#168-external-editor-mode))

## Priority 2 — Core + TUI Changes

Require modifications to the `swink-agent` core crate alongside TUI changes.

- [x] **Tiered Approval Modes** — Add `Smart` mode: auto-approve reads, prompt for writes. Per-tool session trust via "always approve." `#approve smart`. ([PRD §16.11](docs/planning/PRD.md#1611-tiered-approval-modes))
- [x] **Plan Mode** — Read-only mode restricting agent to read-only tools. Toggle via `Shift+Tab` or `/plan`. Status bar indicator. Switch to execute mode to act on the plan. ([PRD §16.9](docs/planning/PRD.md#169-plan-mode))

## Priority 3 — New Rendering Subsystem

Requires new UI components, interaction patterns, and tool integration.

- [x] **Inline Diff View** — Syntax-highlighted unified/side-by-side diffs for file modifications. Per-hunk approve/reject. Adaptive layout by terminal width. ([PRD §16.6](docs/planning/PRD.md#166-inline-diff-view))
