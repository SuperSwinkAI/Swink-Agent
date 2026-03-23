# Tasks: TUI: Input & Conversation

**Input**: Design documents from `/specs/026-tui-input-conversation/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Tests**: Tests are included as the spec explicitly calls for unit tests (`cargo test -p swink-agent-tui`) and specifies test coverage for input editor operations, conversation scroll behavior, markdown parsing, and syntax highlighting fallback.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Path Conventions

- **TUI crate**: `tui/src/` (existing crate in workspace)
- All source files already exist per plan.md; tasks modify existing files

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Verify existing project structure and ensure dependencies are in place

- [x] T001 Verify `ratatui` 0.30, `crossterm` 0.29, and `syntect` 5 dependencies are declared in workspace `Cargo.toml` and `tui/Cargo.toml`
- [x] T002 [P] Verify `#[forbid(unsafe_code)]` is present at `tui/src/lib.rs` crate root
- [x] T003 [P] Verify `tui/src/ui/mod.rs` re-exports `input`, `conversation`, `markdown`, and `syntax` sub-modules

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and theme infrastructure that ALL user stories depend on

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 Implement `MessageRole` enum with variants `User`, `Assistant`, `ToolResult`, `Error`, `System` and derives (`Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`) in `tui/src/app/state.rs`
- [x] T005 Implement `DisplayMessage` struct with fields `role`, `content`, `thinking`, `is_streaming`, `collapsed`, `summary`, `user_expanded`, `expanded_at`, `plan_mode`, `diff_data` and derive (`Debug`, `Clone`) in `tui/src/app/state.rs` (depends on T004 for `MessageRole`)
- [x] T006 [P] Implement role color functions in `tui/src/theme.rs`: `user_color()` -> Green, `assistant_color()` -> Cyan, `tool_color()` -> Yellow, `error_color()` -> Red, `system_color()` -> Magenta, plus `heading_color()` and `inline_code_color()`
- [x] T007 Implement `role_color()` mapping function that returns the correct color for each `MessageRole` variant in `tui/src/theme.rs`
- [x] T008 Re-export `InputEditor`, `ConversationView`, `markdown_to_lines`, and `highlight_code` from `tui/src/lib.rs`

**Checkpoint**: Foundation ready — user story implementation can now begin

---

## Phase 3: User Story 1 — Compose and Submit Messages (Priority: P1) MVP

**Goal**: Developer can type multi-line messages, edit text with cursor movement, submit with Enter, insert newlines with Shift+Enter, and recall history with Up/Down

**Independent Test**: Type text, verify cursor movement and editing, press Shift+Enter for a newline, press Enter to submit, and verify the message content is returned correctly

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T009 [P] [US1] Write unit tests for `InputEditor::new()` (single empty line, cursor at 0,0) and `is_empty()`/`is_multiline()` predicates in `tui/src/ui/input.rs`
- [x] T010 [P] [US1] Write unit tests for `insert_char()`: character insertion at cursor, cursor advancement, insertion mid-line in `tui/src/ui/input.rs`
- [x] T011 [P] [US1] Write unit tests for `insert_newline()`: line splitting at cursor, cursor moves to new line start in `tui/src/ui/input.rs`
- [x] T012 [P] [US1] Write unit tests for `backspace()`: delete before cursor, merge lines at col 0, no-op at (0,0) in `tui/src/ui/input.rs`
- [x] T013 [P] [US1] Write unit tests for `delete()`: delete at cursor, merge with next line at end, no-op at last position in `tui/src/ui/input.rs`
- [x] T014 [P] [US1] Write unit tests for cursor movement: `move_left()` wrap, `move_right()` wrap, `move_up()` clamp, `move_down()` clamp, `move_home()`, `move_end()` in `tui/src/ui/input.rs`
- [x] T015 [P] [US1] Write unit tests for `submit()`: returns joined text, returns `None` for whitespace-only, clears editor, saves to history in `tui/src/ui/input.rs`
- [x] T016 [P] [US1] Write unit tests for `height()`: returns 3 for single line, grows with content, caps at 10 in `tui/src/ui/input.rs`
- [x] T017 [P] [US1] Write unit tests for history: `history_prev()` saves draft and loads entry, `history_next()` restores draft, editing recalled entry does not modify history in `tui/src/ui/input.rs`
- [x] T017a [P] [US1] Write unit tests for Unicode handling: insert emoji, CJK characters, and combining characters via `insert_char()`; verify cursor movement accounts for multi-byte characters in `tui/src/ui/input.rs`

### Implementation for User Story 1

- [x] T018 [US1] Implement `InputEditor` struct with fields (`lines`, `cursor_row`, `cursor_col`, `scroll_offset`, `history`, `history_index`, `saved_input`) and `new()` constructor in `tui/src/ui/input.rs`
- [x] T019 [US1] Implement `height()` method: `(lines.len() + 2).clamp(3, 10)` as `u16` in `tui/src/ui/input.rs`
- [x] T020 [US1] Implement `insert_char()` and `insert_newline()` methods in `tui/src/ui/input.rs`
- [x] T021 [US1] Implement `backspace()` and `delete()` methods with line-merge behavior in `tui/src/ui/input.rs`
- [x] T022 [US1] Implement cursor movement methods (`move_left`, `move_right`, `move_up`, `move_down`, `move_home`, `move_end`) with wrapping and clamping in `tui/src/ui/input.rs`
- [x] T023 [US1] Implement `is_empty()` and `is_multiline()` predicate methods in `tui/src/ui/input.rs`
- [x] T024 [US1] Implement `submit()`: join lines with `\n`, trim, return `None` if empty, push to history, clear editor, reset cursor and history state in `tui/src/ui/input.rs`
- [x] T025 [US1] Implement `history_prev()` and `history_next()`: save/restore draft, navigate history vector, move cursor to end of last line in `tui/src/ui/input.rs`
- [x] T026 [US1] Implement `render()` method: bordered block, line number gutter (multi-line only, right-aligned 2-digit), cursor positioning accounting for gutter width and scroll offset, status hint in title in `tui/src/ui/input.rs`
- [x] T027 [US1] Wire key events to `InputEditor` in `tui/src/app/event_loop.rs`: character input -> `insert_char()`, Enter -> `submit()`, Shift+Enter -> `insert_newline()`, Backspace -> `backspace()`, Delete -> `delete()`, arrow keys -> cursor movement, Home/Ctrl+A -> `move_home()`, End/Ctrl+E -> `move_end()`, Up/Down in empty editor -> history navigation

**Checkpoint**: User Story 1 fully functional — developer can compose, edit, and submit messages with history recall

---

## Phase 4: User Story 2 — View Conversation with Role Formatting (Priority: P1)

**Goal**: Developer sees messages with role-colored left borders, streaming content with cursor indicator, and auto-scroll during streaming

**Independent Test**: Submit a message, verify user message appears with green border, assistant response streams with cyan border and blinking cursor, streaming cursor disappears on completion

### Tests for User Story 2

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T028 [P] [US2] Write unit tests for `ConversationView::new()` (offset 0, auto_scroll true) in `tui/src/ui/conversation.rs`
- [x] T029 [P] [US2] Write unit tests for auto-scroll behavior: render sets offset to bottom when auto_scroll is true in `tui/src/ui/conversation.rs`

### Implementation for User Story 2

- [x] T030 [US2] Implement `ConversationView` struct with fields (`scroll_offset`, `auto_scroll`, `rendered_lines`) and `const fn new()` constructor in `tui/src/ui/conversation.rs`
- [x] T031 [US2] Implement `render()` method: iterate messages, render role header line (bold label), colored left border (`│ `) per `MessageRole`, content via `markdown_to_lines()`, blank line between messages in `tui/src/ui/conversation.rs`
- [x] T032 [US2] Implement streaming cursor indicator: append blinking block cursor (`█`) to last line of streaming messages when `blink_on && is_streaming`, remove when `is_streaming` becomes false in `tui/src/ui/conversation.rs`
- [x] T033 [US2] Implement auto-scroll in `render()`: when `auto_scroll` is true, set `scroll_offset` to `rendered_lines - inner_height` each frame in `tui/src/ui/conversation.rs`

**Checkpoint**: User Story 2 fully functional — messages display with role colors, streaming shows cursor indicator, auto-scroll keeps latest content visible

---

## Phase 5: User Story 3 — Scroll Through Conversation History (Priority: P2)

**Goal**: Developer can scroll up/down through long conversations, auto-scroll pauses during manual scroll, "scroll to bottom" indicator appears, auto-scroll resumes when scrolled to bottom

**Independent Test**: Generate long conversation, scroll up, verify indicator appears and auto-scroll pauses, scroll to bottom and verify auto-scroll resumes

### Tests for User Story 3

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T034 [P] [US3] Write unit tests for `scroll_up()`: decrements offset (saturating at 0), disengages auto-scroll in `tui/src/ui/conversation.rs`
- [x] T035 [P] [US3] Write unit tests for `scroll_down()`: increments offset, re-engages auto-scroll at bottom in `tui/src/ui/conversation.rs`
- [x] T036 [P] [US3] Write unit tests for `scroll_to_bottom()`: sets offset to max, re-engages auto-scroll in `tui/src/ui/conversation.rs`
- [x] T037 [P] [US3] Write unit tests for `clamp_scroll_offset()`: ensures offset <= rendered_lines - visible_height in `tui/src/ui/conversation.rs`

### Implementation for User Story 3

- [x] T038 [US3] Implement `scroll_up()`: decrement by `n` (saturating), set `auto_scroll = false` in `tui/src/ui/conversation.rs`
- [x] T039 [US3] Implement `scroll_down()`: increment by `n`, clamp to max, set `auto_scroll = true` if at bottom in `tui/src/ui/conversation.rs`
- [x] T040 [US3] Implement `clamp_scroll_offset()` and `scroll_to_bottom()` in `tui/src/ui/conversation.rs`
- [x] T041 [US3] Add "scroll to bottom" indicator in conversation title when `auto_scroll` is false and offset < max in `tui/src/ui/conversation.rs`
- [x] T042 [US3] Wire scroll key events in `tui/src/app/event_loop.rs`: Up/Down -> `scroll_up(1)`/`scroll_down(1)`, PageUp/PageDown -> `scroll_up(page)`/`scroll_down(page)` when conversation view has focus

**Checkpoint**: User Story 3 fully functional — developer can scroll through history, auto-scroll pauses/resumes correctly, indicator visible when scrolled up

---

## Phase 6: User Story 4 — Read Formatted Markdown in Responses (Priority: P2)

**Goal**: Assistant responses with markdown formatting render with visual emphasis: headers, bold, italic, inline code, fenced code blocks, bullet/numbered lists, and word-wrapping

**Independent Test**: Have agent produce response with headers, bold, code blocks, and lists; verify each is rendered with appropriate visual treatment

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T043 [P] [US4] Write unit tests for `markdown_to_lines()` with empty input (returns empty vec) in `tui/src/ui/markdown.rs`
- [x] T044 [P] [US4] Write unit tests for header parsing: `#`, `##`, `###` rendered bold with heading color, `#` also underlined in `tui/src/ui/markdown.rs`
- [x] T045 [P] [US4] Write unit tests for inline formatting: `**bold**` -> BOLD modifier, `*italic*` -> ITALIC modifier, `` `code` `` -> inline code color + BOLD in `tui/src/ui/markdown.rs`
- [x] T046 [P] [US4] Write unit tests for fenced code blocks: language label passed to `highlight_code()`, unclosed blocks flushed at end in `tui/src/ui/markdown.rs`
- [x] T047 [P] [US4] Write unit tests for bullet lists (`- `, `* ` -> Unicode bullet + 2-space indent) and numbered lists (`N. ` -> preserved number + 2-space indent) in `tui/src/ui/markdown.rs`
- [x] T048 [P] [US4] Write unit tests for word-wrapping: long lines split at word boundaries to fit width, empty lines preserved in `tui/src/ui/markdown.rs`

### Implementation for User Story 4

- [x] T049 [US4] Implement `markdown_to_lines()` entry point function with block-level state machine tracking `in_code_block`, `code_lang`, `code_buffer` in `tui/src/ui/markdown.rs`
- [x] T050 [US4] Implement header detection and rendering: `#`/`##`/`###` lines rendered with heading color, bold modifier, `#` also underlined in `tui/src/ui/markdown.rs`
- [x] T051 [US4] Implement `parse_inline()` private function: parse `**bold**`, `*italic*`, `` `code` `` within a line, returning styled `Span`s in `tui/src/ui/markdown.rs`
- [x] T052 [US4] Implement bullet and numbered list detection: `- `/`* ` -> Unicode bullet prefix with 2-space indent, `N. ` -> preserved number with 2-space indent in `tui/src/ui/markdown.rs`
- [x] T053 [US4] Implement `wrap_spans()` and `split_preserving_spaces()` private functions for word-wrapping styled spans to fit width in `tui/src/ui/markdown.rs`
- [x] T054 [US4] Implement fenced code block handling: detect ` ``` ` open/close, buffer lines, dispatch to `syntax::highlight_code()` on close, flush unclosed blocks at end in `tui/src/ui/markdown.rs`

**Checkpoint**: User Story 4 fully functional — markdown in responses is visually formatted with headers, emphasis, code, lists, and word-wrapping

---

## Phase 7: User Story 5 — View Syntax-Highlighted Code Blocks (Priority: P3)

**Goal**: Fenced code blocks with language labels are syntax-highlighted; unrecognized or missing labels fall back to plain monospace

**Independent Test**: Have agent produce a labeled code block, verify language keywords are colored differently from strings and comments

### Tests for User Story 5

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T055 [P] [US5] Write unit tests for `highlight_code()` with recognized language (e.g., "rust"): returns non-empty lines with styled spans in `tui/src/ui/syntax.rs`
- [x] T056 [P] [US5] Write unit tests for `highlight_code()` with unrecognized language: returns plain DIM lines in `tui/src/ui/syntax.rs`
- [x] T057 [P] [US5] Write unit tests for `highlight_code()` with empty language string: returns plain DIM lines in `tui/src/ui/syntax.rs`

### Implementation for User Story 5

- [x] T058 [US5] Implement `syntax_set()` private function: `OnceLock`-cached `SyntaxSet::load_defaults_newlines()` in `tui/src/ui/syntax.rs`
- [x] T059 [US5] Implement `theme_set()` private function: `OnceLock`-cached `ThemeSet::load_defaults()` in `tui/src/ui/syntax.rs`
- [x] T060 [US5] Implement `to_ratatui_color()` private function: convert `syntect::Color` to `ratatui::Color::Rgb` in `tui/src/ui/syntax.rs`
- [x] T061 [US5] Implement `highlight_code()` public function: look up syntax by token, highlight with theme, convert to `Line`s with 2-space indent prefix, fall back to plain DIM text for unrecognized/empty language in `tui/src/ui/syntax.rs`
- [x] T062 [US5] Implement monochrome mode check: skip syntect entirely when `color_mode() != Custom`, render plain DIM text in `tui/src/ui/syntax.rs`

**Checkpoint**: User Story 5 fully functional — code blocks with recognized languages are syntax-highlighted, others fall back gracefully

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Improvements that affect multiple user stories

- [x] T063 [P] Verify all public types are re-exported from `tui/src/lib.rs` per contract
- [x] T064 [P] Run `cargo clippy -p swink-agent-tui -- -D warnings` and fix any warnings
- [x] T065 Run `cargo test -p swink-agent-tui` and verify all tests pass
- [x] T066 Run quickstart.md code examples as validation: `InputEditor` compose/submit, cursor movement, `ConversationView` scroll, `markdown_to_lines`, `highlight_code`
- [x] T067 Verify scrolling performance with 500+ messages does not introduce perceptible lag
- [x] T068 [P] Validate large paste handling: insert 10,000+ characters into `InputEditor`, verify no panic or performance degradation in `tui/src/ui/input.rs`

---

## Dependencies & Execution Order

### Phase Dependencies

> **Note**: Phase numbers reflect user story priority order, not execution order. The dependency graph and recommended execution order below take precedence. For example, US4 (Phase 6) should be implemented before US2 (Phase 4) because US2 depends on US4's `markdown_to_lines()`.

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational (Phase 2) — no other story dependencies
- **US2 (Phase 4)**: Depends on Foundational (Phase 2) and US4 (Phase 6, for `markdown_to_lines()` in render)
- **US3 (Phase 5)**: Depends on US2 (Phase 4, ConversationView must exist for scroll methods)
- **US4 (Phase 6)**: Depends on Foundational (Phase 2) — no other story dependencies
- **US5 (Phase 7)**: Depends on Foundational (Phase 2) — no other story dependencies (US4 calls `highlight_code` but can stub)
- **Polish (Phase 8)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (Compose/Submit)**: Independent — only needs foundational types
- **US2 (Conversation View)**: Needs `markdown_to_lines()` from US4 and `highlight_code()` from US5 for full rendering; can stub initially
- **US3 (Scroll History)**: Needs `ConversationView` from US2
- **US4 (Markdown)**: Independent — pure function, no dependencies on other stories
- **US5 (Syntax Highlighting)**: Independent — pure function, no dependencies on other stories

### Recommended Execution Order

1. Phase 1 (Setup) + Phase 2 (Foundational)
2. US1 (Phase 3) + US4 (Phase 6) + US5 (Phase 7) — **in parallel** (independent)
3. US2 (Phase 4) — depends on US4/US5 output
4. US3 (Phase 5) — depends on US2
5. Phase 8 (Polish)

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Struct/type definitions before methods
- Core logic before rendering
- Story complete before moving to next priority

### Parallel Opportunities

**Phase 2 (Foundational)**:
- T004 then T005 (same file, T005 depends on T004); T006 can run in parallel with T004/T005 (different file)

**US1 (Phase 3) Tests**:
- T009 through T017 can ALL run in parallel (all test functions in same file, no ordering dependency)

**US4 (Phase 6) Tests**:
- T043 through T048 can ALL run in parallel

**US5 (Phase 7) Tests**:
- T055 through T057 can ALL run in parallel

**Cross-Story Parallelism**:
- US1 (Phase 3), US4 (Phase 6), and US5 (Phase 7) can be implemented entirely in parallel after Foundational completes

---

## Parallel Example: User Story 1

```bash
# Launch all tests for US1 together (all [P] marked):
Task T009: "Unit tests for new() and predicates in tui/src/ui/input.rs"
Task T010: "Unit tests for insert_char() in tui/src/ui/input.rs"
Task T011: "Unit tests for insert_newline() in tui/src/ui/input.rs"
Task T012: "Unit tests for backspace() in tui/src/ui/input.rs"
Task T013: "Unit tests for delete() in tui/src/ui/input.rs"
Task T014: "Unit tests for cursor movement in tui/src/ui/input.rs"
Task T015: "Unit tests for submit() in tui/src/ui/input.rs"
Task T016: "Unit tests for height() in tui/src/ui/input.rs"
Task T017: "Unit tests for history in tui/src/ui/input.rs"
```

---

## Parallel Example: Independent User Stories

```bash
# After Phase 2 completes, these three stories can run in parallel:
# Developer A: US1 — InputEditor (tui/src/ui/input.rs)
# Developer B: US4 — Markdown renderer (tui/src/ui/markdown.rs)
# Developer C: US5 — Syntax highlighting (tui/src/ui/syntax.rs)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: User Story 1 (Compose and Submit)
4. **STOP and VALIDATE**: Test InputEditor independently
5. Developer can compose, edit, and submit messages

### Incremental Delivery

1. Complete Setup + Foundational -> Foundation ready
2. Add US1 (Compose/Submit) -> Test independently (MVP!)
3. Add US4 (Markdown) + US5 (Syntax) -> Test independently (parallel)
4. Add US2 (Conversation View) -> Test independently -> Full rendering
5. Add US3 (Scroll History) -> Test independently -> Full interaction
6. Each story adds value without breaking previous stories

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together
2. Once Foundational is done:
   - Developer A: US1 (InputEditor) in `tui/src/ui/input.rs`
   - Developer B: US4 (Markdown) in `tui/src/ui/markdown.rs`
   - Developer C: US5 (Syntax) in `tui/src/ui/syntax.rs`
3. After those complete:
   - Developer A: US2 (ConversationView) in `tui/src/ui/conversation.rs`
   - Developer B: US3 (Scroll History) in `tui/src/ui/conversation.rs`
4. Polish phase

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story should be independently completable and testable
- Verify tests fail before implementing
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- All source files already exist — tasks modify existing files, no new files created
