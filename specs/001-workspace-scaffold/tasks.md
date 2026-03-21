# Tasks: Workspace & Cargo Scaffold

**Input**: Design documents from `/specs/001-workspace-scaffold/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Workspace Root Configuration)

**Purpose**: Create the root workspace configuration files that all crates depend on

- [x] T001 Create root `Cargo.toml` with: (a) `[workspace]` defining 7 members (`.`, `adapters`, `memory`, `local-llm`, `eval`, `tui`, `xtask`); (b) `[workspace.dependencies]` centralizing all shared dependency versions (serde, serde_json, tokio, futures, thiserror, uuid, reqwest, jsonschema, schemars, rand, tracing, toml) plus internal crates (`swink-agent.path = "."`, etc.); (c) `[workspace.lints]` with `unsafe_code = "forbid"` and clippy all+pedantic+nursery as warnings with targeted allows (`module_name_repetitions`, `must_use_candidate`, `missing_errors_doc`, `missing_panics_doc`); (d) `[package]` for `swink-agent` core (version 0.1.0, edition 2024, rust-version 1.88); (e) `[features]` section with `default = ["builtin-tools"]`, `builtin-tools = []`, and `test-helpers = []` (disabled by default — gates shared test utilities for downstream crates); (f) dev and release `[profile]` sections (split-debuginfo dev, LTO release)
- [x] T002 [P] Create `rust-toolchain.toml` pinning `channel = "1.88"`
- [x] T003 [P] Create `rustfmt.toml` with project formatting rules
- [x] T004 [P] Create `.gitignore` excluding `target/`, `.env`, editor files, OS artifacts

**Checkpoint**: Root configuration complete — crate scaffolding can begin

---

## Phase 2: Core Crate (US1 — Library Consumer Adds Dependency)

**Goal**: A developer adds `swink-agent` as a dependency and their project compiles. The crate exposes a clean, minimal public surface.

**Independent Test**: `cargo build -p swink-agent` and `cargo build -p swink-agent --no-default-features` both succeed

### Implementation

- [x] T005 [US1] Create `src/lib.rs` with `#![forbid(unsafe_code)]`, conditional `mod builtin_tools;` gated by `#[cfg(feature = "builtin-tools")]`, and a placeholder public re-export comment documenting the future public API surface
- [x] T006 [US1] Verify core crate compiles with default features (`cargo build -p swink-agent`) and without (`cargo build -p swink-agent --no-default-features`)

**Checkpoint**: Core crate compiles — dependent crates can now be scaffolded

---

## Phase 3: Dependent Library Crates (US2 — Workspace Build, US3 — Adapter Author)

**Goal**: All library crates compile as workspace members with correct inter-crate dependencies. No crate depends on a crate it shouldn't.

**Independent Test**: `cargo build --workspace` succeeds with zero errors

### Implementation

- [x] T007 [P] [US2] Create `adapters/Cargo.toml` for `swink-agent-adapters` (lib) depending on `swink-agent.workspace = true`, inheriting workspace lints; create `adapters/src/lib.rs` with `#![forbid(unsafe_code)]` and stub re-exports
- [x] T008 [P] [US2] Create `memory/Cargo.toml` for `swink-agent-memory` (lib) depending on `swink-agent.workspace = true`, inheriting workspace lints; create `memory/src/lib.rs` with `#![forbid(unsafe_code)]` and stub re-exports
- [x] T009 [P] [US2] Create `local-llm/Cargo.toml` for `swink-agent-local-llm` (lib) depending on `swink-agent.workspace = true`, inheriting workspace lints; create `local-llm/src/lib.rs` with `#![forbid(unsafe_code)]` and stub re-exports
- [x] T010 [P] [US2] Create `eval/Cargo.toml` for `swink-agent-eval` (lib) depending on `swink-agent.workspace = true`, inheriting workspace lints; create `eval/src/lib.rs` with `#![forbid(unsafe_code)]` and stub re-exports
- [x] T011 [P] [US2] Create `tui/Cargo.toml` for `swink-agent-tui` (bin+lib) depending on `swink-agent`, `swink-agent-adapters`, `swink-agent-memory`, `swink-agent-local-llm` (all `.workspace = true`), inheriting workspace lints; create `tui/src/lib.rs` with `#![forbid(unsafe_code)]` and stub re-exports; create `tui/src/main.rs` with minimal entry point
- [x] T012 [P] [US2] Create `xtask/Cargo.toml` for `xtask` (bin) with no production dependencies, inheriting workspace lints; create `xtask/src/main.rs` with empty `fn main() {}`
- [x] T013 [US2] Create `tests/common/mod.rs` with placeholder shared test helpers (empty `MockStreamFn`, `MockTool` stubs as comments documenting future contents). This module is gated by the `test-helpers` feature flag defined in T001 — downstream crates enable `swink-agent/test-helpers` in their `[dev-dependencies]` to access these utilities

**Checkpoint**: All 7 crates compile as workspace members

---

## Phase 4: User Story 2 — Workspace Developer Builds All Crates (Priority: P1)

**Goal**: Contributor clones repo, runs workspace build/lint, zero errors and zero warnings

### Verification

- [x] T014 [US2] Run `cargo build --workspace` — verify all 7 crates compile with zero errors
- [x] T015 [US2] Run `cargo clippy --workspace -- -D warnings` — verify zero warnings
- [x] T016 [US2] Run `cargo test --workspace` — verify test harness passes (no business-logic tests expected yet)
- [x] T017 [US2] Run `cargo fmt --check` — verify formatter produces no diffs

**Checkpoint**: Workspace-wide build, lint, test, and format all pass

---

## Phase 5: User Story 3 — Adapter Author Adds a New Provider (Priority: P2)

**Goal**: Adapters crate depends only on core, not on memory/eval/local-llm/TUI

### Verification

- [x] T018 [US3] Inspect `adapters/Cargo.toml` dependency list — confirm it depends only on `swink-agent` and no other workspace crates
- [x] T019 [US3] Inspect `tui/Cargo.toml` dependency list — confirm it depends on core, adapters, memory, and local-llm as expected by FR-010

**Checkpoint**: Dependency boundaries verified

---

## Phase 6: User Story 4 — Toolchain Consistency (Priority: P2)

**Goal**: Toolchain, formatter, and linter produce consistent results across environments

### Verification

- [x] T020 [US4] Verify `rust-toolchain.toml` causes `rustup` to select Rust 1.88 (check `rustc --version` output after entering the workspace)
- [x] T021 [US4] Verify each crate individually builds: `cargo build -p swink-agent-adapters`, `cargo build -p swink-agent-memory`, `cargo build -p swink-agent-local-llm`, `cargo build -p swink-agent-eval`, `cargo build -p swink-agent-tui`, `cargo build -p xtask`

**Checkpoint**: Toolchain consistency verified

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Final validation across all acceptance scenarios

- [x] T022 Run `quickstart.md` validation — execute all commands from quickstart.md and verify they succeed
- [x] T023 Verify SC-005: no crate depends on a crate it should not (review all `Cargo.toml` dependency sections against data-model.md dependency graph)
- [x] T024 Verify SC-006: `swink-agent` alone does not transitively pull in adapters, memory, eval, local-llm, or TUI dependencies (`cargo tree -p swink-agent` should show only core deps)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Setup)**: No dependencies — start immediately
- **Phase 2 (Core Crate)**: Depends on T001 (root Cargo.toml)
- **Phase 3 (Dependent Crates)**: Depends on Phase 2 (core must exist for workspace deps)
- **Phase 4–6 (Verification)**: Depends on Phase 3 completion
- **Phase 7 (Polish)**: Depends on all prior phases

### Parallel Opportunities

- T002, T003, T004 can run in parallel (independent config files)
- T007–T012 can all run in parallel (independent crate directories, no cross-dependencies at scaffold time)
- T014–T017 can run in parallel (independent verification commands)
- T018–T019 can run in parallel (independent inspections)

### Critical Path

```
T001 → T005 → T007–T012 (parallel) → T014–T017 (parallel) → T022–T024
```

---

## Notes

- No business logic in any crate — only structural scaffolding per FR-012
- All library crates must have `#![forbid(unsafe_code)]` per FR-011
- The `builtin-tools` feature flag is structural only — the gated module is empty in this scaffold
- Verification phases (4–6) are lightweight since this is a scaffold — they confirm structural correctness
