# Tasks: TUI: Commands, Editor & Session

**Input**: Design documents from `/specs/028-tui-commands-editor-session/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md, quickstart.md

**Tests**: Tests are included per user story. Unit tests in the respective source modules; integration tests in `tui/src/app/tests/` [corrected 2026-07-06: the former monolithic `tui/src/app/tests.rs` was split into `tui/src/app/tests/{mod,approval,helpers,input_ui,persistence,plan_mode,tool_blocks,agent_bridge}.rs`].

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

**Important notes**:
- All source files (`commands.rs`, `editor.rs`, `session.rs`) already exist as scaffolded modules. Tasks update them to match the contract.
- Session persistence delegates entirely to `swink-agent-memory`'s `SessionStore`/`JsonlSessionStore`. The TUI re-exports these types; no new persistence logic is needed.
- Clipboard operations use the `arboard` crate (already in `tui/Cargo.toml`). The `ClipboardBridge` concept is implemented inline in the event loop, not as a separate struct.
- The `/model` command is intentionally omitted from the command parser — model switching is handled via F4 key cycling. `/model` returns an "Unknown command" feedback per the implementation.
- The `#info` command returns an empty `Feedback(String)` — the caller (event loop) fills in session info via `self.session_info()`.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Dependencies & Configuration)

**Purpose**: Ensure workspace dependencies and Cargo.toml are ready

- [x] T001 [P] Verify `arboard = "3"` is listed in `tui/Cargo.toml` under `[dependencies]` for cross-platform clipboard access
- [x] T002 [P] Verify `swink-agent-memory` path dependency is listed in `tui/Cargo.toml` for session persistence re-exports
- [x] T003 [P] Verify `keyring = "3"` is listed in `tui/Cargo.toml` for credential storage (`#key` command)
- [x] T004 [P] Verify `crossterm` has `event-stream` feature enabled in `tui/Cargo.toml` for async terminal event handling

---

## Phase 2: Foundation (Core Types & Enums)

**Purpose**: Define the `CommandResult`, `ClipboardContent`, and `ApprovalModeArg` enums that all command handling depends on.

- [x] T005 Define `CommandResult` enum in `tui/src/commands.rs` with all variants: `Feedback(String)`, `Quit`, `Clear`, `SetThinking(String)`, `SetSystemPrompt(String)`, `Reset`, `CopyToClipboard(ClipboardContent)`, `SaveSession`, `LoadSession(String)`, `ListSessions`, `StoreKey { provider, key }`, `ListKeys`, `SetApprovalMode(ApprovalModeArg)`, `QueryApprovalMode`, `OpenEditor`, `TogglePlanMode`, `ToggleHelp`, `NotACommand`. Derive `Debug`.
- [x] T006 [P] Define `ClipboardContent` enum in `tui/src/commands.rs`: `Last`, `All`, `Code`. Derive `Debug, Clone, Copy`.
- [x] T007 [P] Define `ApprovalModeArg` enum in `tui/src/commands.rs`: `On`, `Off`, `Smart`. Derive `Debug, Clone, Copy, PartialEq, Eq`.

**Checkpoint**: Core types defined — command parser and event loop can now reference them

---

## Phase 3: User Story 1 — Execute Hash Commands for In-Session Actions (Priority: P1)

**Goal**: Developer types `#help`, `#clear`, `#info`, `#copy`, `#copy all`, `#copy code`, `#approve on/off/smart`, `#save`, `#load <id>`, `#sessions`, `#key`, `#keys` and gets correct `CommandResult` variants.

**Independent Test**: Type `#help` → `ToggleHelp`. Type `#clear` → `Clear`. Type `#copy code` → `CopyToClipboard(Code)`. Type `#nonexistent` → `Feedback` with error.

### Tests for User Story 1

> **NOTE: Write tests FIRST, ensure they FAIL before implementation.**

- [x] T008 [P] [US1] Test `plain_text_is_not_a_command` in `tui/src/commands.rs` tests: `execute_command("hello world")` → `NotACommand`
- [x] T009 [P] [US1] Test `empty_input_is_not_a_command` in `tui/src/commands.rs` tests: `execute_command("")` → `NotACommand`
- [x] T010 [P] [US1] Test `whitespace_only_is_not_a_command` in `tui/src/commands.rs` tests: `execute_command("   ")` → `NotACommand`
- [x] T011 [P] [US1] Test `hash_help_toggles_panel` in `tui/src/commands.rs` tests: `execute_command("#help")` → `ToggleHelp`
- [x] T012 [P] [US1] Test `hash_clear_returns_clear` in `tui/src/commands.rs` tests: `execute_command("#clear")` → `Clear`
- [x] T013 [P] [US1] Test `hash_info_returns_feedback` in `tui/src/commands.rs` tests: `execute_command("#info")` → `Feedback(_)`
- [x] T014 [P] [US1] Test `hash_copy_variants` in `tui/src/commands.rs` tests: `#copy` → `CopyToClipboard(Last)`, `#copy all` → `CopyToClipboard(All)`, `#copy code` → `CopyToClipboard(Code)`
- [x] T015 [P] [US1] Test `hash_sessions_returns_list_sessions` in `tui/src/commands.rs` tests: `execute_command("#sessions")` → `ListSessions`
- [x] T016 [P] [US1] Test `hash_save_returns_save_session` in `tui/src/commands.rs` tests: `execute_command("#save")` → `SaveSession`
- [x] T017 [P] [US1] Test `hash_load_with_id` in `tui/src/commands.rs` tests: `execute_command("#load abc123")` → `LoadSession("abc123")`
- [x] T018 [P] [US1] Test `hash_load_without_id_returns_feedback` in `tui/src/commands.rs` tests: `execute_command("#load")` → `Feedback` with "Unknown command"
- [x] T019 [P] [US1] Test `hash_key_with_provider_and_key` in `tui/src/commands.rs` tests: `execute_command("#key openai sk-abc123")` → `StoreKey { provider: "openai", key: "sk-abc123" }`
- [x] T020 [P] [US1] Test `hash_key_without_key_returns_usage` in `tui/src/commands.rs` tests: `execute_command("#key openai")` → `Feedback` with "Usage"
- [x] T021 [P] [US1] Test `hash_keys_returns_list_keys` in `tui/src/commands.rs` tests: `execute_command("#keys")` → `ListKeys`
- [x] T022 [P] [US1] Test `hash_approve_query` in `tui/src/commands.rs` tests: `execute_command("#approve")` → `QueryApprovalMode`
- [x] T023 [P] [US1] Test `hash_approve_on/off/smart` in `tui/src/commands.rs` tests: three cases for `SetApprovalMode` variants
- [x] T024 [P] [US1] Test `hash_approve_invalid_arg_returns_usage` in `tui/src/commands.rs` tests: `execute_command("#approve maybe")` → `Feedback` with "Usage"
- [x] T025 [P] [US1] Test `hash_unknown_command_returns_feedback` in `tui/src/commands.rs` tests: `execute_command("#nonexistent")` → `Feedback` with "Unknown command"
- [x] T026 [P] [US1] Test `leading_trailing_whitespace_trimmed` in `tui/src/commands.rs` tests: `"  #clear  "` → `Clear`, `"  /quit  "` → `Quit`

### Implementation for User Story 1

- [x] T027 [US1] Implement `execute_command(input: &str) -> CommandResult` in `tui/src/commands.rs`: trim input, dispatch to `execute_hash_command` if `#` prefix, `execute_slash_command` if `/` prefix, else `NotACommand`
- [x] T028 [US1] Implement `execute_hash_command(cmd: &str) -> CommandResult` in `tui/src/commands.rs`: match on `help`, `clear`, `info`, `copy`, `copy all`, `copy code`, `sessions`, `save`, `keys`, `load <id>`, `key <provider> <key>`, `approve [on|off|smart]`, else unknown command feedback
- [x] T029 [US1] Wire hash command results in `tui/src/app/event_loop.rs` `submit_input()`: handle `ToggleHelp`, `Clear`, `Feedback`, `SaveSession`, `LoadSession`, `ListSessions`, `StoreKey`, `ListKeys`, `SetApprovalMode`, `QueryApprovalMode` variants

**Checkpoint**: US1 complete — all hash commands parse and dispatch correctly

---

## Phase 4: User Story 2 — Execute Slash Commands for System Actions (Priority: P1)

**Goal**: Developer types `/quit`, `/thinking <level>`, `/system <prompt>`, `/reset`, `/editor`, `/plan` and gets correct `CommandResult` variants.

**Independent Test**: Type `/quit` → `Quit`. Type `/system You are a pirate.` → `SetSystemPrompt("You are a pirate.")`. Type `/nonexistent` → `Feedback` with error.

### Tests for User Story 2

- [x] T030 [P] [US2] Test `slash_quit` in `tui/src/commands.rs` tests: `execute_command("/quit")` → `Quit`
- [x] T031 [P] [US2] Test `slash_q_alias` in `tui/src/commands.rs` tests: `execute_command("/q")` → `Quit`
- [x] T032 [P] [US2] Test `slash_thinking_with_arg` in `tui/src/commands.rs` tests: `execute_command("/thinking high")` → `SetThinking("high")`
- [x] T033 [P] [US2] Test `slash_thinking_without_arg_returns_usage` in `tui/src/commands.rs` tests: `execute_command("/thinking")` → `Feedback` with "Usage"
- [x] T034 [P] [US2] Test `slash_system_with_arg` in `tui/src/commands.rs` tests: `execute_command("/system You are a pirate.")` → `SetSystemPrompt("You are a pirate.")`
- [x] T035 [P] [US2] Test `slash_system_without_arg_returns_usage` in `tui/src/commands.rs` tests: `execute_command("/system")` → `Feedback` with "Usage"
- [x] T036 [P] [US2] Test `slash_reset` in `tui/src/commands.rs` tests: `execute_command("/reset")` → `Reset`
- [x] T037 [P] [US2] Test `slash_editor` in `tui/src/commands.rs` tests: `execute_command("/editor")` → `OpenEditor`
- [x] T038 [P] [US2] Test `slash_plan` in `tui/src/commands.rs` tests: `execute_command("/plan")` → `TogglePlanMode`
- [x] T039 [P] [US2] Test `slash_unknown_command_returns_feedback` in `tui/src/commands.rs` tests: `execute_command("/nonexistent")` → `Feedback` with "Unknown command"
- [x] T040 [P] [US2] Test `slash_model_is_unknown_command` in `tui/src/commands.rs` tests: `execute_command("/model gpt-4o")` → `Feedback` (model switching is via F4)

### Implementation for User Story 2

- [x] T041 [US2] Implement `execute_slash_command(cmd: &str) -> CommandResult` in `tui/src/commands.rs`: split command name from args at first space, match on `quit`/`q`, `thinking`, `system`, `reset`, `editor`, `plan`, else unknown command feedback
- [x] T042 [US2] Wire slash command results in `tui/src/app/event_loop.rs` `submit_input()`: handle `Quit` (set `should_quit`), `SetThinking` (feedback), `SetSystemPrompt` (update agent), `Reset` (clear all state), `OpenEditor` (set flag), `TogglePlanMode` (toggle mode)

**Checkpoint**: US2 complete — all slash commands parse and dispatch correctly

---

## Phase 5: User Story 3 — Compose Messages in External Editor (Priority: P2)

**Goal**: Developer invokes `/editor`, TUI suspends, editor opens with temp file, content is submitted on close, empty file cancels.

**Independent Test**: Set `EDITOR=true`, run `/editor` → editor exits with empty file → cancellation message. Set `EDITOR=__nonexistent__` → error message.

### Tests for User Story 3

- [x] T043 [P] [US3] Test `resolve_editor_with_config_override` in `tui/src/editor.rs` tests: `resolve_editor(Some("nano"))` → `"nano"`
- [x] T044 [P] [US3] Test `resolve_editor_falls_back_to_vi` in `tui/src/editor.rs` tests: `resolve_editor(None)` → non-empty string (env-dependent)
- [x] T045 [P] [US3] Test `open_editor_with_nonexistent_command` in `tui/src/editor.rs` tests: `open_editor("__nonexistent_editor_binary_12345__")` → `Err`
- [x] T046 [P] [US3] Test `open_editor_with_true_command_returns_none` in `tui/src/editor.rs` tests: `open_editor("true")` → `Ok(None)` (empty file = cancellation)

### Implementation for User Story 3

- [x] T047 [US3] Implement `resolve_editor(config_override: Option<&str>) -> String` in `tui/src/editor.rs`: check config override, then `$EDITOR`, then `$VISUAL`, then `"vi"` fallback
- [x] T048 [US3] Implement `open_editor(editor_command: &str) -> io::Result<Option<String>>` in `tui/src/editor.rs`: create temp file via `tempfile::NamedTempFile::new()?.into_temp_path()` [corrected 2026-07-06: randomized temp path, not a deterministic `{temp_dir}/swink-prompt-{pid}.md` name], launch editor via `std::process::Command`, read result, clean up in all paths (success, error, cancel)
- [x] T049 [US3] Wire editor in event loop (`tui/src/app/event_loop.rs`): when `open_editor_requested` is set, suspend TUI via `restore_terminal()`, launch editor, resume TUI, submit content or show cancel/error feedback. Recreate event stream after resume.

**Checkpoint**: US3 complete — external editor round-trip works end-to-end

---

## Phase 6: User Story 4 — Save and Restore Conversation Sessions (Priority: P2)

**Goal**: Developer saves conversation via `#save`, lists via `#sessions`, loads via `#load <id>`. Session persistence uses `swink-agent-memory`'s `JsonlSessionStore`.

**Independent Test**: Have a conversation, run `#save`, quit, relaunch, run `#load <id>`, verify history restored.

### Tests for User Story 4

- [x] T050 [P] [US4] Test `save_session` in `tui/src/app/tests/` [corrected 2026-07-06: no test named exactly `save_session` was found under `tui/src/app/tests/*.rs`; the closest coverage is `auto_save_session()` call sites in `tui/src/app/tests/input_ui.rs` and `tui/src/app/tests/persistence.rs` (e.g. `auto_save_persists_session_state_snapshot`) — this task may have landed under a different name or scope]: create App with `JsonlSessionStore` in tempdir, push messages, call `save_session()`, verify file created and feedback message shown
- [x] T051 [P] [US4] Test `load_session` in `tui/src/app/tests/` [corrected 2026-07-06: no test named exactly `load_session`; related coverage exists as `load_session_restores_error_messages_with_role_and_content` and other `load_session_*` tests in `tui/src/app/tests/persistence.rs`, `load_session_keeps_full_agent_state_but_trims_visible_history` in `tui/src/app/tests/input_ui.rs`, and `resume_into_loads_existing_session` in `tui/src/app/tests/input_ui.rs`]: save a session, clear app, call `load_session(id)`, verify messages restored and session_id updated
- [x] T052 [P] [US4] Test `list_sessions` in `tui/src/app/tests/` [corrected 2026-07-06: no test named `list_sessions` or exercising `list_sessions()` was found under `tui/src/app/tests/*.rs`; this task may not have landed as described]: save a session, call `list_sessions()`, verify feedback contains session ID
- [x] T053 [P] [US4] Test `load_nonexistent_session` in `tui/src/app/tests/` [corrected 2026-07-06: no test named exactly `load_nonexistent_session`; the closest match is `resume_into_errors_on_missing_session` in `tui/src/app/tests/input_ui.rs`]: call `load_session("nonexistent")`, verify error feedback shown

### Implementation for User Story 4

- [x] T054 [US4] Implement `session.rs` re-exports in `tui/src/session.rs`: `pub use swink_agent_memory::{JsonlSessionStore, SessionMeta, SessionStore}`
- [x] T055 [US4] Add `session_store: Option<JsonlSessionStore>` and `session_id: String` fields to `App` struct in `tui/src/app/state.rs`
- [x] T056 [US4] Implement `auto_save_session()` in `tui/src/app/persistence.rs`: extract `LlmMessage` from agent state, build `SessionMeta`, call `store.save()`
- [x] T057 [US4] Implement `save_session()` in `tui/src/app/persistence.rs`: delegate to `auto_save_session()`, show confirmation feedback
- [x] T058 [US4] Implement `load_session(id: &str)` in `tui/src/app/persistence.rs`: call `store.load()`, replace conversation messages with loaded `DisplayMessage` entries, restore model name, rebuild conversation view, sync agent state via `agent.set_messages()`
- [x] T059 [US4] Implement `list_sessions()` in `tui/src/app/persistence.rs`: call `store.list()`, format session metadata as feedback text with "#load <id>" hint

**Checkpoint**: US4 complete — session save/load/list cycle works

---

## Phase 7: User Story 5 — Copy Conversation Content to Clipboard (Priority: P3)

**Goal**: Developer uses `#copy`, `#copy all`, `#copy code` to place content on system clipboard with confirmation feedback.

**Independent Test**: Generate a response with code blocks, run `#copy code`, paste elsewhere to verify.

### Tests for User Story 5

- [x] T060 [P] [US5] Test `no_code_blocks_returns_none` in `tui/src/app/render_helpers.rs` tests: text with no fenced blocks → `None`
- [x] T061 [P] [US5] Test `single_code_block` in `tui/src/app/render_helpers.rs` tests: single fenced block → extracted content
- [x] T062 [P] [US5] Test `multiple_code_blocks_returns_last` in `tui/src/app/render_helpers.rs` tests: three blocks → last block content
- [x] T063 [P] [US5] Test `unterminated_code_block` in `tui/src/app/render_helpers.rs` tests: unclosed fence → `None`
- [x] T064 [P] [US5] Test `empty_code_block` in `tui/src/app/render_helpers.rs` tests: ```` ``` ``` ```` → `Some("")`
- [x] T065 [P] [US5] Test `code_block_with_language_tag` in `tui/src/app/render_helpers.rs` tests: ```` ```rust ``` ```` → content without language tag

### Implementation for User Story 5

- [x] T066 [US5] Implement `extract_last_code_block(text: &str) -> Option<String>` in `tui/src/app/render_helpers.rs`: scan for fenced code blocks (``` delimiters), extract inner content, return last block
- [x] T067 [US5] Implement `copy_to_clipboard(content: ClipboardContent)` in `tui/src/app/event_loop.rs`: for `Last` find last assistant message, for `All` format all messages as `"{role}: {content}\n\n"`, for `Code` extract code blocks via `extract_last_code_block`. Use `arboard::Clipboard::new()` + `set_text()`, show confirmation or error feedback.

**Checkpoint**: US5 complete — clipboard copy with all three modes works

---

## Phase 8: Cross-Cutting — Credential Management & Approval Wiring

**Purpose**: Wire `#key`, `#keys`, `#approve` command results to app actions

- [x] T068 [P] Implement `store_key(provider, key)` in `tui/src/app/persistence.rs`: delegate to `credentials::store_credential()`, show success/error feedback
- [x] T069 [P] Implement `list_keys()` in `tui/src/app/persistence.rs`: call `credentials::check_credentials()` and `credentials::providers()`, format status table with `✓`/`✗` icons
- [x] T070 Wire `SetApprovalMode` handling in `tui/src/app/event_loop.rs`: map `On`→`Enabled`, `Off`→`Bypassed`, `Smart`→`Smart`, set on App and Agent, show feedback
- [x] T071 Wire `QueryApprovalMode` handling in `tui/src/app/event_loop.rs`: format current mode label, include trusted tools list if Smart mode

**Checkpoint**: All cross-cutting command integrations wired

---

## Phase 9: Polish & Integration Verification

**Purpose**: Verify end-to-end behavior across all command categories

- [x] T072 [P] Verify `tui/src/lib.rs` correctly declares `mod commands`, `mod editor`, `mod session` as private modules (public API is through `app`)
- [x] T073 [P] Verify `tui/src/commands.rs` `execute_command` is `pub` (used by `app::event_loop`)
- [x] T074 [P] Verify `tui/src/editor.rs` `resolve_editor` and `open_editor` are `pub` (used by `app::event_loop`)
- [x] T075 [P] Verify `ApprovalModeArg` has `Debug, Clone, Copy, PartialEq, Eq` derives per contract (test in `approval_mode_arg_debug_and_eq`)
- [x] T076 Run `cargo test -p swink-agent-tui` — all command, editor, render_helpers, and app tests pass
- [x] T077 Run `cargo clippy -p swink-agent-tui -- -D warnings` — zero warnings
- [x] T078 Run `cargo build -p swink-agent-tui` — clean build with no errors

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Setup)**: No dependencies — verify existing Cargo.toml
- **Phase 2 (Foundation)**: No dependencies — define enums
- **Phase 3 (US1 Hash Commands)**: Depends on Phase 2 — needs CommandResult enum
- **Phase 4 (US2 Slash Commands)**: Depends on Phase 2 — needs CommandResult enum
- **Phase 5 (US3 Editor)**: Depends on Phase 4 — needs `/editor` → `OpenEditor` dispatch
- **Phase 6 (US4 Sessions)**: Depends on Phase 3 — needs `#save`/`#load`/`#sessions` dispatch
- **Phase 7 (US5 Clipboard)**: Depends on Phase 3 — needs `#copy` dispatch
- **Phase 8 (Cross-Cutting)**: Depends on Phases 3 + 4 — needs full command dispatch
- **Phase 9 (Polish)**: Depends on all previous phases

### Parallel Opportunities

- Phases 3 (US1) and 4 (US2) can proceed in parallel — hash and slash commands are independent parsers
- Phases 5, 6, and 7 can proceed in parallel after their respective dependencies
- All test tasks marked [P] within a phase can run in parallel
