# Feature Specification: TUI: Input & Conversation

**Feature Branch**: `026-tui-input-conversation`
**Created**: 2026-03-20
**Status**: Draft
**Input**: Multi-line input editor, scrollable conversation view, markdown rendering, syntax highlighting for code blocks. References: PRD §16.2-16.3 (Rendering, Interaction), HLD TUI Component Model, TUI_PHASES T2.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Compose and Submit Messages (Priority: P1)

A developer types a message in the input editor and submits it to the agent. The editor supports multi-line input: pressing Enter submits the message, while Shift+Enter inserts a newline. The editor grows dynamically in height as the developer types (minimum 3 lines, maximum 10 lines), showing a line number gutter for orientation. Standard text editing works: character insertion and deletion, cursor movement with arrow keys, Home/End or Ctrl+A/Ctrl+E to jump to line start/end. The developer can recall previously submitted messages using Up/Down arrow keys when the editor is empty.

**Why this priority**: The input editor is the primary interaction point — without it, the developer cannot communicate with the agent.

**Independent Test**: Can be tested by typing text, verifying cursor movement and editing, pressing Shift+Enter for a newline, pressing Enter to submit, and verifying the message appears in the conversation.

**Acceptance Scenarios**:

1. **Given** an empty input editor, **When** the developer types characters, **Then** they appear at the cursor position.
2. **Given** text in the editor, **When** Enter is pressed, **Then** the message is submitted and the editor clears.
3. **Given** text in the editor, **When** Shift+Enter is pressed, **Then** a newline is inserted and the editor height increases.
4. **Given** the editor has 3 lines, **When** more lines are added beyond 10, **Then** the editor height caps at 10 lines and scrolls internally.
5. **Given** previous messages were submitted, **When** the developer presses Up in an empty editor, **Then** the most recent previous message is recalled into the editor.
6. **Given** a recalled message, **When** the developer presses Down, **Then** the next message in history is shown (or the editor clears if at the end).
7. **Given** the cursor is mid-line, **When** Home or Ctrl+A is pressed, **Then** the cursor jumps to the start of the line.

---

### User Story 2 - View Conversation with Role Formatting (Priority: P1)

A developer views the ongoing conversation between themselves and the agent. Each message has a colored left border indicating its role: green for user messages, cyan for assistant messages, yellow for tool results, red for errors, and magenta for system messages. As new messages arrive (including streaming assistant responses), the view auto-scrolls to show the latest content. A streaming cursor indicator (e.g., a blinking block) shows that the assistant is still generating.

**Why this priority**: The conversation view is the primary output surface — the developer must see agent responses to use the tool.

**Independent Test**: Can be tested by sending a message, verifying the user message appears with a green border, the assistant response streams in with a cyan border, and the view auto-scrolls.

**Acceptance Scenarios**:

1. **Given** a user message is submitted, **When** it appears in the conversation, **Then** it has a green left border.
2. **Given** an assistant response is streaming, **When** tokens arrive, **Then** they appear incrementally with a cyan left border and a streaming cursor indicator.
3. **Given** a tool result arrives, **When** it appears in the conversation, **Then** it has a yellow left border.
4. **Given** an error occurs, **When** the error message appears, **Then** it has a red left border.
5. **Given** the conversation has more content than fits on screen, **When** new content arrives, **Then** the view auto-scrolls to the bottom.
6. **Given** streaming completes, **When** the final token arrives, **Then** the streaming cursor indicator disappears.

---

### User Story 3 - Scroll Through Conversation History (Priority: P2)

A developer wants to review earlier parts of a long conversation. They scroll up using Up/Down arrow keys (when the conversation view has focus), PageUp/PageDown for larger jumps, or the mouse wheel. When the developer has scrolled away from the bottom, a "scroll to bottom" indicator appears. Auto-scroll is paused while the developer is manually scrolling — new content does not yank the viewport away. When the developer scrolls back to the bottom (or activates the indicator), auto-scroll resumes.

**Why this priority**: Reviewing history is important for understanding context, but secondary to seeing current responses.

**Independent Test**: Can be tested by generating a long conversation, scrolling up, verifying the indicator appears and auto-scroll pauses, then scrolling to the bottom and verifying auto-scroll resumes.

**Acceptance Scenarios**:

1. **Given** a long conversation, **When** the developer presses Up or PageUp on the conversation view, **Then** the view scrolls toward earlier messages.
2. **Given** the view is scrolled away from the bottom, **When** new content arrives, **Then** auto-scroll is paused and the viewport stays in place.
3. **Given** the view is scrolled up, **When** looking at the UI, **Then** a "scroll to bottom" indicator is visible.
4. **Given** the view is scrolled up, **When** the developer scrolls back to the bottom, **Then** auto-scroll resumes and the indicator disappears.
5. **Given** a long conversation, **When** the developer rolls the mouse wheel over the conversation view, **Then** the view scrolls by a fixed step and behaves like keyboard scrolling.

---

### User Story 3a - Select and Copy Chat Content (Priority: P2)

A developer wants to capture text from the conversation — an error message, a snippet of assistant output, a path — without copying the entire message. Pressing and holding the primary mouse button inside the conversation view anchors a selection at the pointer cell; dragging extends it cell-by-cell across rows (including multi-line selections). The selected cells are visually highlighted (reversed video). Releasing the button copies the selected text to the system clipboard. Pressing Ctrl+C while a selection is active also copies it; Escape clears the selection. The scroll wheel keeps working throughout, and rolling the wheel clears any active selection.

Because the TUI owns mouse capture for scroll-wheel support, the terminal emulator cannot do its own click-drag selection. This user story provides an in-app equivalent so copying behaves naturally without requiring terminal-specific modifier bypasses (Shift/Option/Fn drag) — those continue to work on terminals that support them.

**Why this priority**: Copy-out of chat content is a common interaction that significantly reduces friction when collaborating on errors and snippets, but is not required to operate the agent.

**Independent Test**: Can be tested by clicking inside the conversation view, dragging across a multi-line range, verifying the cells are highlighted, releasing the button, and confirming the clipboard contains the selected text with trailing whitespace and the role-border gutter stripped.

**Acceptance Scenarios**:

1. **Given** the conversation view has visible content, **When** the developer presses the left mouse button inside the viewport, **Then** a selection is anchored at that cell.
2. **Given** an anchored selection, **When** the developer drags the mouse to another cell inside the viewport, **Then** the highlighted range extends from the anchor to the current cell in row-major order.
3. **Given** an anchored selection, **When** the developer drags the pointer outside the viewport, **Then** the selection clamps to the viewport edges and keeps extending.
4. **Given** an active selection, **When** the developer releases the left mouse button, **Then** the selected text is written to the system clipboard (with the role-colored gutter prefix stripped per line and trailing whitespace removed).
5. **Given** an active selection, **When** the developer presses Ctrl+C, **Then** the selected text is copied to the clipboard and the selection clears.
6. **Given** an active selection, **When** the developer presses Escape, **Then** the selection clears without copying.
7. **Given** an active selection, **When** the developer rolls the mouse wheel or begins a new click-drag, **Then** the existing selection clears before the new interaction proceeds.
8. **Given** the clipboard is unavailable (e.g., headless environment), **When** a copy is attempted, **Then** a non-fatal system message is surfaced and the TUI does not crash.

---

### User Story 4 - Read Formatted Markdown in Responses (Priority: P2)

A developer reads assistant responses that contain markdown formatting. Headers are rendered with visual emphasis (size or style differentiation). Bold and italic text are styled appropriately. Inline code is visually distinct from surrounding text. Fenced code blocks are rendered in a monospace style with clear boundaries. Bullet and numbered lists are properly indented and prefixed. Long lines are word-wrapped to fit the available width without horizontal scrolling.

**Why this priority**: Markdown rendering significantly improves readability but the conversation is usable with plain text.

**Independent Test**: Can be tested by having the agent produce a response with headers, bold, code blocks, and lists, then verifying each is rendered with the appropriate visual treatment.

**Acceptance Scenarios**:

1. **Given** a response with `# Header`, **When** rendered, **Then** the header is visually prominent (larger or styled differently from body text).
2. **Given** a response with `**bold**` and `*italic*`, **When** rendered, **Then** bold text is bold and italic text is italic.
3. **Given** a response with `` `inline code` ``, **When** rendered, **Then** it is displayed in a distinct style from prose.
4. **Given** a response with a fenced code block, **When** rendered, **Then** the block has clear visual boundaries and uses monospace rendering.
5. **Given** a response with a bullet list, **When** rendered, **Then** each item is indented with a bullet prefix.
6. **Given** a long line of text, **When** rendered, **Then** the text word-wraps at the viewport boundary without horizontal scrolling.

---

### User Story 5 - View Syntax-Highlighted Code Blocks (Priority: P3)

A developer reads assistant responses containing fenced code blocks with language labels (e.g., ` ```python `). The code within the block is syntax-highlighted with colors corresponding to language constructs (keywords, strings, comments, etc.). When no language label is provided, the block falls back to plain monospace rendering without highlighting. The highlighting enhances readability without being distracting.

**Why this priority**: Syntax highlighting is a polish feature that improves code readability but is not required for basic functionality.

**Independent Test**: Can be tested by having the agent produce a response with a labeled code block, verifying that language keywords are colored differently from strings and comments.

**Acceptance Scenarios**:

1. **Given** a fenced code block with a language label, **When** rendered, **Then** language constructs are syntax-highlighted with distinct colors.
2. **Given** a fenced code block without a language label, **When** rendered, **Then** the code is displayed in plain monospace without highlighting.
3. **Given** a code block with an unrecognized language label, **When** rendered, **Then** the code falls back to plain monospace rendering.
4. **Given** a code block that is wider than the viewport, **When** rendered, **Then** long lines wrap or are scrollable within the block boundary.

---

### Edge Cases

- What happens when the developer pastes a very large block of text (10,000+ chars) — no max input size enforced; handled as a text buffer insertion.
- How does the input editor handle non-ASCII characters — Rust/ratatui handle Unicode natively; emoji, CJK, combining characters work.
- What happens when conversation contains hundreds of messages — scrolling uses indexed offsets, not full traversal; remains responsive.
- How does the markdown renderer handle malformed/nested markdown — hand-rolled markdown parser tolerates malformed input gracefully.
- What happens when streaming response contains partial markdown — text accumulates during streaming; markdown is re-rendered on each update.
- How does the conversation view handle a single extremely long line — word-wrap logic fits text to viewport width.
- What happens when the developer submits an empty message — `input.trim().is_empty()` check prevents submission of whitespace-only messages.
- How does input history behave when a recalled message is edited then Up pressed — editing a recalled message leaves history unmodified; next Up shows the next history item.
- **TurnEnd bridge**: When the agent emits `TurnEnd` events containing tool results, the TUI appends each tool result message to the conversation display. The TUI then trims the in-memory conversation history to the 20 most recent turns to bound memory usage in long-running sessions.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The input editor MUST support character insertion, deletion, and cursor movement (arrow keys, Home/End, Ctrl+A/Ctrl+E).
- **FR-002**: The input editor MUST dynamically resize between a minimum of 3 lines and a maximum of 10 lines based on content.
- **FR-003**: The input editor MUST display a line number gutter when the editor contains 2 or more lines.
- **FR-004**: Enter MUST submit the message; Shift+Enter MUST insert a newline.
- **FR-005**: The input editor MUST support input history recall via Up/Down arrow keys when the editor is empty.
- **FR-006**: The conversation view MUST display messages with role-colored left borders: green (user), cyan (assistant), yellow (tool), red (error), magenta (system).
- **FR-007**: The conversation view MUST auto-scroll to new content unless the user has manually scrolled away from the bottom.
- **FR-008**: The conversation view MUST display a streaming cursor indicator during active streaming.
- **FR-009**: The conversation view MUST display a "scroll to bottom" indicator when the user has scrolled away from the latest content.
- **FR-010**: The conversation view MUST support scrolling via Up/Down and PageUp/PageDown keys.
- **FR-011**: The markdown renderer MUST support headers, bold, italic, inline code, fenced code blocks, and bullet/numbered lists.
- **FR-012**: The markdown renderer MUST word-wrap text to fit the available viewport width.
- **FR-013**: Fenced code blocks with recognized language labels MUST be syntax-highlighted.
- **FR-014**: Fenced code blocks without a language label or with an unrecognized label MUST fall back to plain monospace rendering.
- **FR-015**: Empty or whitespace-only messages MUST NOT be submitted.
- **FR-016**: When the developer submits a message while the agent is already running (streaming), the message MUST be queued as a steering event and delivered to the agent at the next turn boundary — it MUST NOT produce an error and MUST NOT be lost.
- **FR-017**: A queued message MUST NOT appear in the main conversation view while the agent is still streaming the current response. It MUST appear in the conversation in correct chronological order once the agent finishes the current turn and picks up the steering input.
- **FR-018**: While a message is queued (not yet delivered), the TUI MUST display a "Queued" banner above the input editor, showing the pending message text. The banner MUST fade out briefly after the message is delivered, then disappear.
- **FR-019**: The conversation view MUST support mouse-wheel scrolling with the same semantics as keyboard scrolling (auto-scroll disengages on manual scroll up; re-engages when the user scrolls back to the bottom).
- **FR-020**: The conversation view MUST support in-app click-drag text selection: pressing the primary mouse button anchors a selection at the pointer cell inside the viewport; dragging extends the selection in row-major order; releasing copies the selected text to the system clipboard via the clipboard bridge.
- **FR-021**: While an in-app selection is active, Ctrl+C MUST copy the selected text and clear the selection; Escape MUST clear the selection without copying. When no selection is active, Ctrl+C and Escape MUST retain their existing behavior (abort/quit, modal dismiss).
- **FR-022**: Starting a new click-drag, rolling the mouse wheel, or clicking outside the conversation viewport MUST clear any active selection before the new interaction proceeds.
- **FR-023**: Selection drags MUST clamp to the viewport edges when the pointer leaves the conversation area so that the highlight continues to extend to the edge.
- **FR-024**: Copied selection text MUST reflect exactly what is visible on screen (wrapped lines included), with the role-colored gutter prefix (`│ `) stripped per line and trailing whitespace removed.
- **FR-025**: When the system clipboard is unavailable, a failed selection copy MUST surface a non-fatal system message and MUST NOT crash the TUI.

### Key Entities

- **InputEditor**: The multi-line text editor component where the developer composes messages. Manages cursor position, text buffer, dynamic height, line numbers, and input history.
- **ConversationView**: The scrollable panel displaying the sequence of messages between the user and agent. Manages scroll position, auto-scroll behavior, and role-based visual styling.
- **MessageRole**: The classification of a message's origin (user, assistant, tool, error, system) that determines its visual treatment.
- **MarkdownRenderer**: The component that transforms markdown text into styled terminal output, handling headers, emphasis, code, lists, and word wrapping.
- **InputHistory**: An ordered collection of previously submitted messages that can be navigated with Up/Down keys for recall.
- **SteeredMessageOverlay**: A transient banner rendered above the input editor while mid-stream submissions are queued in `pending_steered`. Fades out after delivery via `steered_fade_ticks` countdown.
- **Selection**: In-app click-drag text selection within the conversation viewport. Holds an anchor cell, a cursor cell, and a dragging flag. Coordinates are `(row, col)` inside the conversation's inner area (after the border). Selection text is extracted from per-row cell symbols captured during the same render pass that applies the visual highlight, so the copy reflects the wrapped on-screen content.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A developer can compose a multi-line message and submit it in under 5 seconds for a typical prompt.
- **SC-002**: All five message roles are visually distinguishable by their left border colors.
- **SC-003**: Auto-scroll keeps the latest content visible during streaming without manual intervention.
- **SC-004**: Markdown-formatted responses are rendered with visual differentiation for all supported elements (headers, bold, italic, code, lists).
- **SC-005**: Scrolling through a conversation with 500+ messages has no perceptible lag.
- **SC-006**: Previously submitted messages can be recalled from history with Up/Down keys.
- **SC-007**: Click-drag selection places the selected conversation text on the system clipboard on all supported platforms, with the role-colored gutter stripped. When the clipboard is unavailable the TUI surfaces a system message instead of crashing.

## Clarifications

### Session 2026-03-20

- Q: Very large paste (10,000+ chars)? → A: No max size enforced; handled as text buffer.
- Q: Non-ASCII characters? → A: Rust/ratatui handle Unicode natively.
- Q: Scrolling with hundreds of messages? → A: Indexed offsets; remains responsive.
- Q: Malformed/nested markdown? → A: Hand-rolled markdown parser tolerates gracefully.
- Q: Partial markdown during streaming? → A: Text accumulates; markdown re-rendered each update.
- Q: Extremely long line? → A: Word-wrap fits to viewport width.
- Q: Empty/whitespace submission? → A: `trim().is_empty()` check prevents it.
- Q: Edited recalled message then Up? → A: History unmodified; next Up shows next item.

### Session 2026-04-17

- Q: How should users copy text out of the chat view when the TUI owns mouse capture for scroll? → A: In-app click-drag selection. The TUI tracks anchor + cursor cells inside the conversation viewport, renders a reversed-video highlight, and on mouse-up (or Ctrl+C) writes the selected text to the clipboard via `arboard`. Terminal-native bypasses (Shift/Option/Fn drag) continue to work on terminals that support them.
- Q: Should the selection follow content when the user scrolls mid-selection? → A: No — scroll-wheel clears the selection. Simpler model, and the user can re-select against the new viewport content.
- Q: Should the copied text include the role-colored gutter `│ `? → A: No — the gutter is a rendering decoration, not part of the message content. It is stripped per line before writing to the clipboard. Trailing whitespace is also trimmed.

## Assumptions

- The TUI scaffold, event loop, and focus management from spec 025 are in place.
- The input editor and conversation view are separate components that receive focus via the focus management system.
- The conversation view receives messages and streaming events from the agent event system established in spec 025.
- Syntax highlighting requires a library of language grammars; the set of supported languages is determined during implementation.
- The markdown renderer handles the subset of markdown commonly produced by LLM assistants (not the full CommonMark specification).
- Input history is per-session and not persisted across TUI restarts (session persistence is handled in a separate spec).
