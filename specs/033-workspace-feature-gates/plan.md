# Implementation Plan: Workspace Feature Gates

**Branch**: `033-workspace-feature-gates` | **Date**: 2026-03-25 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/033-workspace-feature-gates/spec.md`

## Summary

Add granular Cargo feature flags across two workspace crates (adapters, local-llm) so consumers compile only the LLM providers and inference backends they need. Follows the proven `swink-agent-policies` pattern: `default = ["all"]`, individual marker flags, `cfg(feature = "...")` guards on module declarations and re-exports. The root `swink-agent` crate is unchanged — consumers depend on sub-crates directly for adapter and backend selection (root cannot depend on adapters due to cyclic dependency). No runtime behavior changes — purely additive compile-time gating.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: Workspace deps centralized in root Cargo.toml. Key deps for this feature: `llama-cpp-2` (backend features), `eventsource-stream` 0.2 (proxy-only), `sha2` (bedrock-only).
**Storage**: N/A
**Testing**: `cargo test --workspace` + `cargo build` with various feature combinations
**Target Platform**: Cross-platform (macOS/Metal, Windows+Linux/CUDA, all/CPU)
**Project Type**: Library (Rust workspace)
**Performance Goals**: N/A (compile-time only change)
**Constraints**: Must not break existing public API. `#[forbid(unsafe_code)]` on all crate roots. MSRV 1.88.
**Scale/Scope**: 2 crates modified (adapters, local-llm). ~9 new feature flags on adapters, ~6 on local-llm. Root crate unchanged (cyclic dependency prevents root → adapters optional dep).

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | **PASS** | Feature flags are the standard library mechanism for optional compilation. No new crates introduced. |
| II. Test-Driven | **PASS** | Existing tests pass with default features. New CI matrix entries verify minimal feature sets. |
| III. Efficiency & Performance | **PASS** | Feature gates reduce compile time and binary size for selective consumers. Zero runtime overhead. |
| IV. Leverage the Ecosystem | **PASS** | Uses standard Cargo feature flag mechanism. Forwards llama-cpp-2 features rather than reimplementing. |
| V. Provider Agnosticism | **PASS** | Core crate remains provider-free. Adapters are a separate workspace crate — consumers opt-in by adding a direct dependency. |
| VI. Safety & Correctness | **PASS** | `#[forbid(unsafe_code)]` unchanged. No new unsafe paths. |

| Constraint | Status | Notes |
|------------|--------|-------|
| Crate count (8 members) | **PASS** | No new crates. Modifying existing crates only. (Constitution says 7 — stale; policies crate added in 032.) |
| MSRV 1.88 | **PASS** | Feature flags are stable Rust. `dep:` syntax available since 1.60. |
| No global mutable state | **PASS** | No runtime state changes. |

**Post-Phase 1 re-check**: All gates still pass. Root crate is unchanged — consumers depend on sub-crates directly for adapter/backend selection.

## Project Structure

### Documentation (this feature)

```text
specs/033-workspace-feature-gates/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output (feature topology)
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── feature-surface.md
└── tasks.md             # Phase 2 output (/speckit.tasks)
```

### Source Code (repository root)

```text
# Files modified (no new files created)
adapters/Cargo.toml           # Add [features] section, gate eventsource-stream + sha2
adapters/src/lib.rs           # Add cfg(feature) guards on mod + pub use
local-llm/Cargo.toml          # Add backend feature flags forwarding to llama-cpp-2
# Root Cargo.toml and src/lib.rs unchanged — cyclic dependency prevents root → adapters
```

**Structure Decision**: No new files or directories. All changes are to existing Cargo.toml manifests and lib.rs files. The adapters and local-llm crate structures remain identical — only compilation visibility changes.

## Complexity Tracking

No constitution violations. Table not needed.
