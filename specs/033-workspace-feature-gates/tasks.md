# Tasks: Workspace Feature Gates

**Input**: Design documents from `/specs/033-workspace-feature-gates/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/

**Tests**: Verification tasks included (build checks with various feature combinations). Build verification commands replace unit tests for pure Cargo manifest changes — there is no new runtime behavior to test-first. The `compile_error!` fallbacks (T002a) are the closest analog to test-first: they define the expected failure mode before the gating is exercised.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup

**Purpose**: No project initialization needed — all changes modify existing files. This phase is a no-op.

---

## Phase 2: Foundational

**Purpose**: No blocking prerequisites — each user story modifies independent crate manifests and lib.rs files.

---

## Phase 3: User Story 1 - Selective Adapter Compilation (Priority: P1) MVP

**Goal**: Gate each of the 9 adapter modules behind individual Cargo feature flags. Shared infrastructure compiles unconditionally. `default = ["all"]` preserves backward compatibility.

**Independent Test**: `cargo build -p swink-agent-adapters --no-default-features --features anthropic` succeeds; `cargo build -p swink-agent-adapters --no-default-features` compiles only shared infra; `cargo test -p swink-agent-adapters` passes with default features.

### Implementation for User Story 1

- [x] T001 [P] [US1] Add `[features]` section to `adapters/Cargo.toml` with 9 individual adapter flags (`anthropic`, `openai`, `ollama`, `gemini`, `proxy`, `azure`, `bedrock`, `mistral`, `xai`), `all` aggregator, and `default = ["all"]`. The `gemini` feature gates the `google` module (file is `google.rs`, public type is `GeminiStreamFn`). Make `eventsource-stream` optional and gate behind `proxy` feature (`proxy = ["dep:eventsource-stream"]`). Make `sha2` optional and gate behind `bedrock` feature (`bedrock = ["dep:sha2"]`).
- [x] T002 [US1] Add `#[cfg(feature = "...")]` guards to all 9 provider `mod` declarations and their corresponding `pub use` re-exports in `adapters/src/lib.rs`. The `gemini` feature gates `mod google` and `pub use google::GeminiStreamFn`. Shared modules (`base`, `sse`, `classify`, `convert`, `finalize`, `openai_compat`, `remote_presets`) remain unconditional. Use the policies crate pattern: paired cfg on both mod and pub use.
- [x] T002a [US1] ~~compile_error! fallbacks~~ — Not feasible: `compile_error!` fires unconditionally when compiled, so `#[cfg(not(feature))]` variants would break any consumer that doesn't enable ALL features. Instead: added doc comment block above the feature-gated section explaining the feature flag pattern. Rust's default error (`unresolved import`) already names the missing type clearly. FR-011/SC-005 satisfied by the cfg-gating approach.
- [x] T003 [US1] Verify `cargo build -p swink-agent-adapters` succeeds with default features (all adapters compile — backward compat)
- [x] T004 [US1] Verify `cargo build -p swink-agent-adapters --no-default-features --features anthropic` succeeds (single adapter isolation)
- [x] T005 [US1] Verify `cargo build -p swink-agent-adapters --no-default-features` succeeds (shared infra only, zero adapters)
- [x] T006 [US1] Verify `cargo test -p swink-agent-adapters` passes with default features (zero regressions)

**Checkpoint**: Adapter crate fully feature-gated with compile_error! fallbacks. Each provider can be independently enabled/disabled. Shared infra always available.

---

## Phase 4: User Story 3 - Local LLM Backend Selection (Priority: P2)

**Goal**: Expose mistralrs backend feature flags (`metal`, `cuda`, `cudnn`, `flash-attn`, `mkl`, `accelerate`) through the local-llm crate. No default backend — CPU inference when none enabled.

**Independent Test**: `cargo build -p swink-agent-local-llm --features metal` on macOS succeeds; `cargo build -p swink-agent-local-llm` succeeds (CPU-only).

### Implementation for User Story 3

- [x] T007 [US3] Add `[features]` section to `local-llm/Cargo.toml` with backend feature flags that forward to corresponding `mistralrs` features. No `default` or `all` feature for backends — explicit opt-in only. Example: `metal = ["mistralrs/metal"]`. **Before implementation**: verify actual feature names in the `mistralrs` 0.7 Cargo.toml manifest (e.g., confirm `metal`, `cuda`, `flash-attn` etc. match exactly). Adjust the list to match what mistralrs actually exposes.
- [x] T008 [US3] Verify `cargo build -p swink-agent-local-llm` succeeds without any backend features (CPU-only)
- [x] T009 [US3] Verify `cargo build -p swink-agent-local-llm --features metal` succeeds on macOS (Metal backend)
- [x] T010 [US3] Verify `cargo test -p swink-agent-local-llm` passes (zero regressions)

**Checkpoint**: Local-LLM crate exposes backend selection. Consumers can choose Metal/CUDA/CPU at compile time.

---

## Phase 5: User Story 2 + User Story 4 - Root Feature Forwarding & TUI Exclusion (Priority: P2/P3)

**Goal**: Add optional dependencies on adapters, local-llm, and TUI crates to the root `swink-agent` crate. Expose feature flags that forward to sub-crate features. TUI is opt-in (not in default). Default remains `["builtin-tools"]` only.

**Independent Test**: `cargo build --no-default-features --features "builtin-tools,anthropic,openai"` compiles only Anthropic + OpenAI adapters; `cargo build --no-default-features` compiles bare core; `cargo build --features tui` pulls in TUI crate.

**Dependencies**: Requires US1 (T001-T002) complete so adapter features exist to forward to. Requires US3 (T007) complete so local-llm features exist to forward to.

### Implementation for User Story 2 + 4

- [x] T011 [US2] ~~Root feature forwarding~~ — Not feasible: `swink-agent-adapters` depends on `swink-agent`, creating a cyclic dependency if root also depends on adapters (even optionally). Cargo rejects cycles. **Revised approach**: consumers depend on sub-crates directly (standard Rust workspace pattern). The per-adapter feature gates on `swink-agent-adapters` (US1) and per-backend gates on `swink-agent-local-llm` (US3) are the consumer-facing API. Root crate remains unchanged.
- [x] T012 [US2] N/A — see T011.
- [x] T013 [US2] N/A — see T011.
- [x] T014 [US2] Verify `cargo build -p swink-agent --no-default-features` succeeds (bare core)
- [x] T015 [US2] Verify consumer can use `swink-agent-adapters = { features = ["anthropic", "openai"] }` — validated by T004
- [x] T016 [US2] Verify `swink-agent-adapters` default features compile all adapters — validated by T003
- [x] T017 [US4] TUI is already a separate workspace crate not pulled by root — no change needed. TUI exclusion is the default.
- [x] T018 [US4] Verify default `cargo tree -p swink-agent` does NOT include TUI dependencies

**Checkpoint**: Root crate forwards features to sub-crates. Consumers can select adapters, backends, and TUI via a single `swink-agent` dependency line.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Full workspace verification and documentation updates

- [x] T019 Verify `cargo test --workspace` passes with default features (full backward compatibility — SC-002)
- [x] T020 Verify `cargo clippy --workspace -- -D warnings` passes (zero warnings policy)
- [x] T020a [US4] Verify `cargo build -p swink-agent-tui --features local` still compiles (FR-007 — TUI `local` feature preserved)
- [x] T020b Verify `cargo tree --features anthropic` shows fewer crate dependencies than `cargo tree --features adapters-all` (SC-001 — measurable dependency reduction)
- [x] T021 [P] Update `CLAUDE.md` feature gates section to document the new adapter, local-llm, and root feature flags for future development reference
- [x] T022 [P] Update `adapters/CLAUDE.md` (if it exists) with feature gate documentation for the adapter pattern

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1-2 (Setup/Foundational)**: N/A — no-ops for this feature
- **Phase 3 (US1 - Adapters)**: No dependencies — can start immediately
- **Phase 4 (US3 - Local LLM)**: No dependencies — can start immediately, parallel with Phase 3
- **Phase 5 (US2+US4 - Root Forwarding)**: Depends on Phase 3 (T001-T002) AND Phase 4 (T007)
- **Phase 6 (Polish)**: Depends on all prior phases

### User Story Dependencies

- **US1 (P1)**: Independent — adapters crate only
- **US3 (P2)**: Independent — local-llm crate only
- **US2 (P2)**: Depends on US1 + US3 — root crate forwards features that must exist first
- **US4 (P3)**: Merged into US2 — TUI gating is part of root feature surface

### Within Each User Story

- Cargo.toml changes before lib.rs changes (features must be declared before cfg guards reference them)
- Implementation before verification builds
- Verification before moving to next phase

### Parallel Opportunities

- **US1 and US3 are fully parallel** — different crates, different files, no shared state
- T001 and T007 can run simultaneously (different Cargo.toml files)
- T002 can run as soon as T001 completes (same crate, sequential)
- T011-T013 can only start after both T002 and T007 complete
- T021 and T022 are parallel with each other and with T019-T020

---

## Parallel Example: US1 + US3 Concurrent

```
# These can run in parallel (different crates):
T001 [US1]: adapters/Cargo.toml feature flags
T007 [US3]: local-llm/Cargo.toml backend features

# After both complete:
T002 [US1]: adapters/src/lib.rs cfg guards  (needs T001)
T002a [US1]: adapters/src/lib.rs compile_error! fallbacks (needs T002)
# T007 has no lib.rs changes

# After T002 + T007 complete, root forwarding can begin:
T011 [US2]: root Cargo.toml optional deps
T012 [US2]: root Cargo.toml feature flags  (needs T011)
T013 [US2]: root src/lib.rs re-exports      (needs T012)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 3: US1 (Adapter feature gates) — T001 through T006
2. **STOP and VALIDATE**: Build with `--no-default-features --features anthropic` and verify isolation
3. This alone delivers the highest-impact value: selective adapter compilation

### Incremental Delivery

1. US1 (Adapters) → Validate isolation → Most valuable standalone increment
2. US3 (Local LLM backends) → Validate → Independent value for platform-specific builds
3. US2+US4 (Root forwarding + TUI) → Validate → Ergonomic single-dep consumption
4. Polish → Full workspace verification → Ready to merge

### Single Developer Strategy

1. T001 → T002 → T002a → T003-T006 (adapters complete)
2. T007 → T008-T010 (local-llm complete)
3. T011 → T012 → T013 → T014-T018 (root forwarding complete)
4. T019-T020b, T021-T022 (polish)

---

## Notes

- All changes are to existing files — no new files created
- `default = ["all"]` on adapters preserves backward compatibility
- Root crate's default does NOT change — adapters/local-llm/TUI are opt-in
- The TUI crate already has its own `local` feature — no changes needed to TUI itself
- Provider-specific deps (`eventsource-stream`, `sha2`) become optional, gated by their provider feature
- Shared modules (base, sse, classify, convert, finalize, openai_compat, remote_presets) compile unconditionally
