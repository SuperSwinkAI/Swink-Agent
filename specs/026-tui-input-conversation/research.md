# Research: TUI: Input & Conversation

**Feature**: 026-tui-input-conversation | **Date**: 2026-03-20

## Decision 1: Hand-Rolled Input Editor vs. tui-textarea

**Question**: Should the input editor use an existing crate like `tui-textarea` or a hand-rolled implementation?

**Decision**: Hand-rolled `InputEditor` struct managing a `Vec<String>` line buffer with explicit cursor tracking.

**Rationale**: The input editor has specific requirements (dynamic height 3-10 lines, line number gutter only in multi-line mode, history recall via Up/Down, Shift+Enter vs Enter distinction) that would require extensive customization of any generic widget. A hand-rolled implementation gives full control over these behaviors and avoids pulling in a dependency that does more than needed. The editor is small (~300 lines) and well-tested. This aligns with Constitution IV (Leverage the Ecosystem): the crate does not do 80% of what is needed, so wrapping it would add complexity without saving effort.

**Alternatives rejected**:
- *tui-textarea*: Would need to be forked or heavily wrapped to support dynamic height, history recall, and the Enter/Shift+Enter split. The wrapping effort exceeds the effort of a direct implementation.
- *tui-input*: Single-line only; does not support multi-line editing.

## Decision 2: Markdown Rendering — Hand-Rolled Line-by-Line Parser

**Question**: Should the markdown renderer use a CommonMark parsing crate (e.g., `pulldown-cmark`) or a hand-rolled approach?

**Decision**: Hand-rolled line-by-line state machine in `markdown.rs` that handles headers, bold, italic, inline code, fenced code blocks, and bullet/numbered lists.

**Rationale**: LLM output uses a predictable subset of markdown. A full CommonMark parser produces an AST that must then be flattened back into terminal-width lines with styling — a lossy conversion that adds complexity. The hand-rolled parser operates line-by-line, directly producing `ratatui::Line` values with styled `Span`s. It handles unclosed code blocks gracefully (important during streaming, where the closing ``` hasn't arrived yet). At ~260 lines with comprehensive tests, the complexity is manageable. This aligns with Constitution IV: a full parser does significantly more than 80% of what is needed, and the translation layer would be non-trivial.

**Alternatives rejected**:
- *pulldown-cmark*: Produces an event-based AST. Converting AST events to terminal lines requires tracking state (open/close tags, nesting depth, wrapping context) that approximates the complexity of the hand-rolled parser, plus the dependency.
- *termimad*: Renders markdown to terminal but uses its own rendering pipeline, not ratatui widgets. Integration would require extracting styled text and re-wrapping it into ratatui `Line`/`Span` types.

## Decision 3: Syntax Highlighting via syntect with OnceLock Caching

**Question**: How should syntax highlighting be implemented for fenced code blocks?

**Decision**: Use `syntect` with `OnceLock`-cached `SyntaxSet` and `ThemeSet`. Language lookup via `find_syntax_by_token`. Fallback to plain dimmed monospace when the language is unrecognized or in monochrome mode.

**Rationale**: `syntect` is the de-facto Rust crate for syntax highlighting. It bundles Sublime Text grammars covering all common programming languages. `OnceLock` ensures grammars are loaded once (first code block render) and reused for the lifetime of the process. The theme selection (`base16-ocean.dark` for dark, `InspiredGitHub` for light) provides good contrast in terminal environments. Monochrome mode skips syntect entirely, rendering plain `DIM` text — this keeps the monochrome path allocation-free after initial buffer construction.

**Alternatives rejected**:
- *tree-sitter*: More accurate parsing but requires per-language grammar binaries, increasing binary size. syntect's regex-based grammars are sufficient for display-only highlighting.
- *No highlighting*: Code readability suffers significantly without keyword/string/comment differentiation.

## Decision 4: Scroll Management — Offset-Based with Auto-Scroll Toggle

**Question**: How should conversation scrolling be managed, especially during streaming?

**Decision**: `ConversationView` tracks `scroll_offset` (line count from top) and an `auto_scroll` boolean. Auto-scroll engages by default and jumps to bottom each frame. Manual scroll (Up/PageUp) disengages auto-scroll. Scrolling back to the bottom re-engages it. A title indicator ("scroll to bottom") appears when disengaged.

**Rationale**: Offset-based scrolling is simple and works with ratatui's `Paragraph::scroll()`. The auto-scroll toggle prevents the jarring experience of new content yanking the viewport during manual review. Re-engagement at the bottom is intuitive — the user signals "I'm done reviewing" by returning to the latest content. The title-based indicator avoids rendering a separate widget.

**Alternatives rejected**:
- *Virtual scrolling with message-level offsets*: More complex; requires pre-computing message heights. Not needed until message counts exceed thousands.
- *Sticky scroll (always auto-scroll)*: Poor UX when reviewing history during streaming.

## Decision 5: Input History — In-Memory Vec with Saved Draft

**Question**: How should input history recall work?

**Decision**: History is a `Vec<Vec<String>>` (each entry is the multi-line content). When the user presses Up in an empty editor (or at the start of history navigation), the current draft is saved in `saved_input`. Up/Down navigate the history stack. Returning past the most recent entry restores the saved draft. History is per-session (not persisted).

**Rationale**: The saved-draft pattern is standard in shell history (bash, zsh). Storing `Vec<String>` (lines) rather than a flat string preserves multi-line structure. Per-session scope keeps the implementation simple; cross-session persistence is explicitly deferred to a separate spec. Editing a recalled entry does not modify history — this matches user expectations from shell history.

**Alternatives rejected**:
- *Persisted history*: Useful but out of scope per spec assumptions. Would require file I/O and format decisions.
- *Edit-in-place history*: Modifying history entries on recall creates surprising behavior when navigating back.
