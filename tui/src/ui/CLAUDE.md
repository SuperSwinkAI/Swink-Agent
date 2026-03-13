# CLAUDE.md — TUI UI Components

## Scope

`tui/src/ui/` — Rendering components. Layout: conversation (flex-grow), tool panel (optional), input, status bar.

## Lessons Learned

- **syntect caches are `OnceLock`** — load once, zero-copy after. Monochrome modes skip syntect entirely (plain DIM text).
- **Thinking sections are dimmed, not collapsible** — QA finding: docs previously claimed "collapsible" but code never implemented it.
- **Auto-scroll disengages on manual scroll** — re-engages when user scrolls to bottom.
- **Help panel degrades on narrow terminals** — doesn't render below `HELP_PANEL_WIDTH + MIN_CONV_WIDTH`, reappears when terminal widens.
