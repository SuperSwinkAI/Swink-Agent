# Tasks: TUI: Scaffold, Event Loop & Config

**Input**: Design documents from `/specs/025-tui-scaffold-config/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

**Context**: The TUI crate (`swink-agent-tui`) already has substantial implementation from prior specs (026, 028, 030). Many tasks in this spec verify and adjust existing code to match the 025 specification, fill identified gaps, and add missing acceptance tests.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Verify existing project structure and ensure crate manifest matches spec requirements

- [x] T001 Verify `tui/Cargo.toml` declares all required dependencies from plan.md: ratatui 0.30, crossterm 0.29 (event-stream feature), tokio, toml 0.8, dirs 6, keyring 3, thiserror, tracing + tracing-subscriber + tracing-appender in tui/Cargo.toml
- [x] T002 [P] Verify `#[forbid(unsafe_code)]` is present at crate root in tui/src/lib.rs
- [x] T003 [P] Verify workspace member entry for `swink-agent-tui` exists in root Cargo.toml

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types and infrastructure that all user stories depend on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 Verify `TuiError` enum has `Io`, `Agent`, and `Other` variants with correct `#[from]` derives in tui/src/error.rs
- [x] T005 [P] Verify `TuiConfig` struct has all fields from data-model.md (`show_thinking`, `auto_scroll`, `tick_rate_ms`, `default_model`, `theme`, `system_prompt`, `editor_command`, `color_mode`) with `#[serde(default)]` in tui/src/config.rs
- [x] T006 Update `TuiConfig::default()` tick_rate_ms from 100 to 33 to match spec (30 FPS = ~33ms) in tui/src/config.rs
- [x] T007 Update existing `TuiConfig` unit tests to expect `tick_rate_ms = 33` instead of 100 in tui/src/config.rs
- [x] T008 [P] Verify `ColorMode` enum with `Custom`, `MonoWhite`, `MonoBlack` variants and `AtomicU8` global in tui/src/theme.rs
- [x] T009 [P] Verify `Focus` enum with `Input` and `Conversation` variants in tui/src/app/state.rs
- [x] T010 [P] Verify `AgentStatus` enum with `Idle`, `Running`, `Error`, `Aborted` variants in tui/src/app/state.rs
- [x] T011 [P] Verify `OperatingMode` enum with `Execute` and `Plan` variants in tui/src/app/state.rs
- [x] T012 [P] Verify `DisplayMessage` struct has all fields from data-model.md in tui/src/app/state.rs
- [x] T013 [P] Verify `MessageRole` enum with `User`, `Assistant`, `ToolResult`, `Error`, `System` variants in tui/src/app/state.rs
- [x] T014 [P] Verify `ProviderInfo` struct with `name`, `key_name`, `env_var`, `description`, `requires_key` fields and five provider entries in tui/src/credentials.rs

**Checkpoint**: Foundation ready — user story implementation can now begin in parallel

---

## Phase 3: User Story 1 — Launch and Exit the TUI Cleanly (Priority: P1) 🎯 MVP

**Goal**: Terminal setup/restore with panic safety and non-interactive terminal detection

**Independent Test**: Launch the TUI, verify alternate screen appears, quit with Ctrl+Q, verify terminal restored. Separately trigger a panic and verify terminal restoration. Pipe input and verify clear error message.

### Implementation for User Story 1

- [x] T015 [US1] Add non-interactive terminal detection to `main()` — check `std::io::stdout().is_terminal()` before `setup_terminal()`, print error to stderr and exit(1) if not a TTY, in tui/src/main.rs
- [x] T016 [US1] Verify `setup_terminal()` enables raw mode, enters alternate screen, and enables mouse capture in tui/src/lib.rs
- [x] T017 [US1] Verify `restore_terminal()` disables raw mode, leaves alternate screen, and disables mouse capture (idempotent) in tui/src/lib.rs
- [x] T018 [US1] Verify panic hook in `main()` calls `restore_terminal()` before the original hook in tui/src/main.rs
- [x] T019 [US1] Verify `Ctrl+Q` sets `should_quit = true` and the event loop exits cleanly in tui/src/app/event_loop.rs
- [x] T020 [US1] Verify file-based logging is configured with `tracing-appender` rolling daily to `dirs::config_dir()/swink-agent/logs/swink-agent.log` in tui/src/main.rs
- [x] T021 [US1] Add unit test for TTY detection: verify non-TTY stdout produces an error message (test the detection logic, not the actual terminal) in tui/src/app/tests.rs

**Checkpoint**: TUI launches, exits cleanly, handles panics, and rejects non-TTY environments

---

## Phase 4: User Story 2 — Respond to Keyboard Input and Agent Events Simultaneously (Priority: P1)

**Goal**: Async event loop multiplexes terminal input and agent events without blocking

**Independent Test**: Send keyboard events and agent events concurrently, verify both are processed without dropped events or UI freezes

### Implementation for User Story 2

- [x] T022 [US2] Verify event loop in `App::run()` uses `tokio::select!` with four branches: terminal events (EventStream), agent events (mpsc), approval requests (mpsc), tick interval in tui/src/app/event_loop.rs
- [x] T023 [US2] Verify dirty-flag rendering: `terminal.draw()` only called when `self.dirty == true`, flag reset after draw in tui/src/app/event_loop.rs
- [x] T024 [US2] Verify tick interval uses `self.config.tick_rate_ms` for the interval duration in tui/src/app/event_loop.rs
- [x] T025 [US2] Verify agent event handler updates `DisplayMessage` and sets `dirty = true` on streaming content in tui/src/app/agent_bridge.rs
- [x] T026 [US2] Verify `Ctrl+C` aborts running agent (sets `AgentStatus::Aborted`) without quitting the TUI in tui/src/app/event_loop.rs
- [x] T027 [US2] Add unit test: construct App, simulate tick, verify `dirty` flag behavior and `blink_on` toggling in tui/src/app/tests.rs

**Checkpoint**: Event loop processes keyboard and agent events concurrently without blocking

---

## Phase 5: User Story 5 — Set Up Provider Credentials on First Run (Priority: P1)

**Goal**: First-run wizard guides developer through provider selection and credential entry

**Independent Test**: Launch with no credentials, complete wizard, verify next launch connects without re-prompting

### Implementation for User Story 5

- [x] T028 [US5] Verify `any_key_configured()` returns false when no API keys are set (checks all providers with `requires_key`) in tui/src/credentials.rs
- [x] T029 [US5] Verify `credential()` checks env var first then keychain (env var wins) in tui/src/credentials.rs
- [x] T030 [US5] Verify `store_credential()` stores to OS keychain via `keyring::Entry` in tui/src/credentials.rs
- [x] T031 [US5] Verify wizard is launched when `!credentials::any_key_configured()` in `run()` function in tui/src/main.rs
- [x] T032 [US5] Verify `SetupWizard` renders provider selection list and handles key entry in tui/src/wizard.rs
- [x] T033 [US5] Verify provider priority order in `create_options()`: Proxy → OpenAI → Anthropic → Local → Ollama in tui/src/main.rs
- [x] T034 [US5] Add unit test: verify `credential()` returns env var value when both env var and keychain are available (mock env var) in tui/src/credentials.rs

**Checkpoint**: First-run wizard works, credentials persist, provider priority order is correct

---

## Phase 6: User Story 3 — Navigate Between UI Components with Keyboard (Priority: P2)

**Goal**: Tab cycles focus forward through components, visual distinction for focused component

**Independent Test**: Press Tab repeatedly, verify focus cycles Input → Conversation → Input with visual border changes

### Implementation for User Story 3

- [x] T035 [US3] Verify Tab key handler cycles `Focus::Input → Focus::Conversation → Focus::Input` in tui/src/app/event_loop.rs
- [x] T036 [US3] Verify focused component gets distinct visual border (e.g., `Color::White` for focused, `Color::DarkGray` for unfocused) in tui/src/ui/conversation.rs and tui/src/ui/input.rs
- [x] T037 [US3] Verify component-specific shortcuts only fire when that component has focus — check that conversation scroll keys (PageUp/PageDown/j/k) are gated on `Focus::Conversation` in tui/src/app/event_loop.rs
- [x] T038 [US3] Add unit test: construct App, simulate Tab keypress, verify focus changes from Input to Conversation and back in tui/src/app/tests.rs

**Checkpoint**: Focus cycling works, visual distinction is clear, component-specific shortcuts are gated

---

## Phase 7: User Story 4 — Configure Appearance and Behavior via Config File (Priority: P2)

**Goal**: TOML config file customizes appearance and behavior, defaults work out of box

**Independent Test**: Write config file with custom color theme, launch TUI, verify custom colors applied

### Implementation for User Story 4

- [x] T039 [US4] Verify `TuiConfig::load()` reads from `dirs::config_dir()/swink-agent/tui.toml` in tui/src/config.rs
- [x] T040 [US4] Verify `TuiConfig::from_toml()` handles partial overrides (missing fields use defaults) in tui/src/config.rs
- [x] T041 [US4] Verify invalid TOML falls back to full defaults (existing test covers this) in tui/src/config.rs
- [x] T042 [US4] Verify unknown keys are silently ignored (existing test covers this) in tui/src/config.rs
- [x] T043 [US4] Verify `App::new()` applies `config.color_mode` to the global `ColorMode` via `theme::set_color_mode()` in tui/src/app/lifecycle.rs
- [x] T044 [US4] Verify `resolve_system_prompt()` priority chain: explicit > `LLM_SYSTEM_PROMPT` env var > config.system_prompt > default constant in tui/src/lib.rs
- [x] T045 [US4] Verify F3 key handler calls `theme::cycle_color_mode()` and sets `dirty = true` in tui/src/app/event_loop.rs
- [x] T046 [US4] Verify `resolve()` function in theme.rs correctly maps colors through MonoWhite and MonoBlack modes in tui/src/theme.rs

**Checkpoint**: Config file loading works with all edge cases, color theme system is functional

---

## Phase 8: User Story 6 — Handle Terminal Resize (Priority: P3)

**Goal**: Terminal resize recalculates layout and renders minimum size warning when below threshold

**Independent Test**: Launch TUI, resize terminal, verify layout adapts. Resize below 120×30, verify warning overlay

### Implementation for User Story 6

- [x] T047 [US6] Verify crossterm `Event::Resize` is handled in the terminal event handler and sets `dirty = true` in tui/src/app/event_loop.rs
- [x] T048 [US6] Implement minimum terminal size check in `ui::render()` — if terminal dimensions are below 120×30, render a centered warning overlay showing current size and minimum required (120×30), skip normal UI rendering, in tui/src/ui/mod.rs
- [x] T049 [US6] Add constants `MIN_TERMINAL_WIDTH = 120` and `MIN_TERMINAL_HEIGHT = 30` at module level in tui/src/ui/mod.rs
- [x] T050 [US6] Verify normal UI rendering resumes immediately when terminal is resized above threshold (no restart needed) in tui/src/ui/mod.rs
- [x] T051 [US6] Add unit test: verify minimum size check returns true/false for various dimensions in tui/src/app/tests.rs

**Checkpoint**: Resize handling works, minimum size warning is clear and non-blocking

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Final verification and cleanup

- [x] T052 [P] Run `cargo build -p swink-agent-tui` and verify clean compilation with no errors
- [x] T053 [P] Run `cargo clippy -p swink-agent-tui -- -D warnings` and fix any warnings
- [x] T054 Run `cargo test -p swink-agent-tui` and verify all existing and new tests pass
- [x] T055 [P] Verify library API matches contract in specs/025-tui-scaffold-config/contracts/library-api.md: `setup_terminal()`, `restore_terminal()`, `resolve_system_prompt()`, `tui_approval_callback()`, `launch()`, `TuiConfig`, `App` re-exports in tui/src/lib.rs
- [x] T056 Run quickstart.md validation: verify `cargo build -p swink-agent-tui` and `cargo test -p swink-agent-tui` succeed as documented

---

## Phase 10: User Story 7 — Transport Abstraction Layer (Priority: P2)

**Goal**: Introduce `TuiTransport` trait decoupling TUI event loop from direct Agent access

**Independent Test**: Construct `InProcessTransport` with mock agent, verify `send()` and `recv()` produce correct results. Inject mock transport into event loop tests.

### Implementation for User Story 7

- [X] T057 [US7] Create `tui/src/transport.rs` with `TuiTransport` trait: `async fn send(&self, input: UserInput) -> Result<(), TransportError>` and `async fn recv(&mut self) -> Option<AgentEvent>`, plus `UserInput` and `TransportError` types
- [X] T058 [US7] Implement `InProcessTransport` struct in `tui/src/transport.rs` that wraps `mpsc::Sender<AgentEvent>` for receiving events (bridged from existing agent channels) and holds a reference to the agent send path
- [X] T059 [US7] Add `SocketTransport` stub in `tui/src/transport.rs` behind `#[cfg(feature = "remote")]` feature gate (can use `unimplemented!()` or return `TransportError::Unavailable` for now)
- [X] T060 [US7] Add `[features] remote = []` to `tui/Cargo.toml`
- [X] T061 [US7] Register `transport` module in `tui/src/lib.rs` and re-export `TuiTransport`, `InProcessTransport`, `TransportError`, `UserInput` types
- [X] T062 [US7] Add unit test: `InProcessTransport` send produces an agent event via mock agent
- [X] T063 [US7] Add unit test: mock `TuiTransport` implementation verifies trait object usage

**Checkpoint**: Transport trait compiles, InProcessTransport passes tests, SocketTransport stub compiles behind feature gate

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Stories (Phases 3–8)**: All depend on Foundational phase completion
  - US1 (P1), US2 (P1), US5 (P1) can proceed in parallel
  - US3 (P2) and US4 (P2) can proceed in parallel after P1 stories
  - US6 (P3) can start after Foundational
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational — no dependencies on other stories
- **User Story 2 (P1)**: Can start after Foundational — no dependencies on other stories
- **User Story 5 (P1)**: Can start after Foundational — no dependencies on other stories
- **User Story 3 (P2)**: Can start after Foundational — may reference US2 event loop but independently testable
- **User Story 4 (P2)**: Can start after Foundational — independent
- **User Story 6 (P3)**: Can start after Foundational — independent

### Within Each User Story

- Verification tasks (existing code) can run in parallel
- New implementation tasks before their corresponding tests
- All tasks within a story complete before marking story done

### Parallel Opportunities

- T001/T002/T003 (Setup) can run in parallel
- T004–T014 (Foundational) — all verification tasks can run in parallel
- US1, US2, US5 (all P1) can start in parallel after Foundational
- US3, US4 (both P2) can start in parallel after Foundational
- T052/T053/T055 (Polish) can run in parallel

---

## Parallel Example: User Story 1

```bash
# Verification tasks can run in parallel:
Task T016: "Verify setup_terminal() in tui/src/lib.rs"
Task T017: "Verify restore_terminal() in tui/src/lib.rs"
Task T018: "Verify panic hook in tui/src/main.rs"

# Then sequentially:
Task T015: "Add TTY detection in tui/src/main.rs"
Task T021: "Add TTY detection test in tui/src/app/tests.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup verification
2. Complete Phase 2: Foundational verification + tick_rate_ms fix
3. Complete Phase 3: User Story 1 (TTY detection is the main new work)
4. **STOP and VALIDATE**: TUI launches, exits cleanly, handles panics, rejects pipes

### Incremental Delivery

1. Setup + Foundational → Foundation verified
2. US1 (launch/exit) + US2 (event loop) + US5 (credentials) → Core P1 MVP
3. US3 (focus) + US4 (config) → P2 enhancements
4. US6 (resize) → P3 polish
5. Each story adds value without breaking previous stories

### Key New Implementation Items

Most of the TUI code already exists. The primary new work items are:
- **T015**: Non-interactive terminal detection (`is_terminal()` check in `main()`)
- **T006/T007**: Fix `tick_rate_ms` default from 100 → 33
- **T048/T049**: Minimum terminal size warning overlay in `ui::render()`

All other tasks are verification and targeted testing of existing code.

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Most tasks verify existing implementation — the TUI was built incrementally across specs 026/028/030
- Three implementation gaps identified: TTY detection, tick_rate default, minimum size overlay
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
