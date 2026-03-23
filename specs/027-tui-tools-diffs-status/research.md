# Research: TUI: Tool Panel, Diffs & Status Bar

**Feature**: 027-tui-tools-diffs-status | **Date**: 2026-03-22

## R1: Diff Algorithm Selection

**Decision**: Longest Common Subsequence (LCS) via dynamic programming.

**Rationale**: LCS is the standard algorithm behind unified diffs (used by `git diff`, `diff`). It produces intuitive output where common lines anchor the diff and changes are shown as insertions/deletions relative to those anchors. The DP approach is O(m*n) in time and space, which is acceptable for file diffs in agent tool results (typically <1000 lines).

**Alternatives considered**:
- **Myers' diff algorithm** (used by git internally): O(nd) where d is edit distance — faster for similar files but more complex to implement. Not worth the complexity for our use case where diffs are small and computed infrequently.
- **Patience diff**: Better semantic grouping for code diffs but significantly more complex. Overkill for a display-only TUI component.
- **External crate (`similar`, `diff`)**: Would require converting from crate-specific output types to ratatui `Line`/`Span`. The LCS implementation is ~35 lines — wrapping an external crate would be more code than the algorithm itself.

## R2: Spinner Animation Approach

**Decision**: Braille character rotation (⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏) driven by the existing tick timer.

**Rationale**: Braille spinners are compact (single character), visually smooth across the 10-frame cycle, and render correctly in all modern terminal emulators. The tick timer already runs at the configured rate for cursor blink — reusing it for spinner frame advancement avoids adding a separate timer.

**Alternatives considered**:
- **ASCII spinner** (`|/-\`): Only 4 frames, visually jerky.
- **Unicode block elements**: Wider (multi-character), harder to align in a single-column layout.
- **Dots (`⠿` variants)**: Fewer distinct frames, less visually clear rotation.

## R3: Tool Panel Layout Strategy

**Decision**: Docked region above the conversation area with conditional visibility.

**Rationale**: A docked panel provides stable layout — the conversation area simply shrinks when tools are active. This avoids z-ordering complexity (overlays) and inline insertion complications (which would shift conversation content and confuse auto-scroll). The panel height is capped at 10 lines to prevent overwhelming the viewport when many tools run concurrently.

**Alternatives considered**:
- **Inline in conversation**: Tools would appear as part of the message flow. Problem: tool status updates would shift content, breaking scroll position. Also mixes ephemeral status with persistent conversation content.
- **Floating overlay**: Requires z-order management and obscures content underneath. Adds visual complexity without clear benefit.
- **Fixed bottom panel** (above status bar): Would compete with the input editor for bottom-of-screen real estate, and tools logically precede the conversation content they produce.

## R4: Token Format Notation

**Decision**: K (thousands) and M (millions) suffixes with one decimal place below 10K.

**Rationale**: Matches conventions used by Claude Code, GitHub, and other developer tools. One decimal place below 10K (e.g., "4.6K") provides useful precision. Above 10K, rounding to whole K (e.g., "15K") avoids false precision. The M suffix handles the growing context windows of modern models.

**Alternatives considered**:
- **Full numbers with commas** ("12,500"): Takes too much horizontal space in the status bar.
- **SI notation** ("12.5k"): Lowercase k is less visually distinct in a terminal.
- **Abbreviated with decimal everywhere** ("12.5K"): Wastes space for large numbers where the decimal adds no value.

## R5: Context Gauge Color Thresholds

**Decision**: Green (<60%), yellow (60-85%), red (>85%).

**Rationale**: The 60% threshold provides early warning before context becomes constrained. The 85% threshold signals urgent action needed (context compaction or new conversation). These thresholds align with common resource monitoring conventions (disk space, memory gauges).

**Alternatives considered**:
- **50/75/90**: Too late for the yellow warning — by 75%, context-heavy operations may already be failing.
- **70/90**: Only two transitions, less granular warning.
- **Gradient coloring**: More visually informative but significantly more complex to implement with ratatui's discrete color model.

## R6: Auto-Collapse Timing

**Decision**: 10 seconds for tool result blocks; 10 seconds for completed tool panel entries; 2 seconds for resolved approval entries.

**Rationale**: 10 seconds gives the developer enough time to glance at a tool result before it collapses, while keeping the conversation tidy during rapid tool execution. Resolved approvals use a shorter 2-second timeout since they carry minimal information (just "Approved"/"Rejected") and the developer already made the decision.

**Alternatives considered**:
- **Configurable via TUI config**: Added complexity for a setting most users won't change. Can be added later if requested.
- **5 seconds**: Too fast — developers may miss results during multi-tool execution.
- **30 seconds**: Too slow — defeats the purpose of auto-collapse for reducing clutter.

## R7: Sensitive Value Redaction in Tool Approval

**Decision**: Apply `redact_sensitive_values` from `swink-agent` core to tool arguments before displaying in approval prompts.

**Rationale**: Tool arguments may contain API keys, passwords, or other secrets (e.g., a bash command with an inline token). The core library already provides a redaction utility — reusing it ensures consistent behavior and prevents accidental secret exposure in the terminal.

**Alternatives considered**:
- **No redaction**: Unacceptable — secrets would be visible in the terminal and potentially in scrollback history.
- **TUI-specific redaction**: Duplicates logic already in core. Violates DRY.
