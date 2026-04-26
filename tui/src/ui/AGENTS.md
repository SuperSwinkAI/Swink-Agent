# AGENTS.md — TUI UI Components

## Scope

`tui/src/ui/` — Rendering components. Layout: conversation (flex-grow), tool panel (optional), input, status bar.

## Key Invariants

- syntect caches are `OnceLock`. Monochrome modes skip syntect (plain DIM text).
- Thinking sections dimmed, not collapsible. Auto-scroll disengages on manual scroll, re-engages at bottom.
- Help panel hides below `HELP_PANEL_WIDTH + MIN_CONV_WIDTH`, reappears when terminal widens.
- Selection is a post-render buffer pass: `Modifier::REVERSED` on cells in range, `selection_text()` extracts what actually wrapped on screen. Copy strips gutter. Drag clamps to viewport edges.
