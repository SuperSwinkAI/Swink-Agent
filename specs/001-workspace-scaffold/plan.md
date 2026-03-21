# Implementation Plan: Workspace & Cargo Scaffold

**Branch**: `001-workspace-scaffold` | **Date**: 2026-03-20 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/001-workspace-scaffold/spec.md`

## Summary

Establish the 7-crate Cargo workspace structure for the Swink Agent library.
The scaffold produces compilable, lintable crates with correct inter-crate
dependencies, centralized dependency versions, pinned toolchain, and strict
linting — but no business logic. All crates contain only structural
definitions, `#[forbid(unsafe_code)]`, and stub `lib.rs`/`main.rs` entry
points.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: serde, serde_json, tokio, futures, thiserror, uuid, reqwest, jsonschema, schemars, rand, tracing, toml (all centralized in workspace `[workspace.dependencies]`)
**Storage**: N/A (scaffold only)
**Testing**: `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`
**Target Platform**: Cross-platform (Linux, macOS, Windows)
**Project Type**: Library (core) + binary (TUI) + dev tooling (xtask)
**Performance Goals**: N/A (scaffold — no runtime code)
**Constraints**: Zero warnings under clippy::all + pedantic + nursery. Zero unsafe code in library crates.
**Scale/Scope**: 7 crates, ~20 files total for scaffold

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | ✅ Pass | Core is a library crate; no service/daemon. Each crate is independently compilable. |
| II. Test-Driven Development | ✅ Pass | Scaffold includes test infrastructure (`tests/common/mod.rs`). TDD cycle applies to subsequent features. |
| III. Efficiency & Performance | ✅ Pass | Build profiles configured (split-debuginfo dev, LTO release). No runtime code in scaffold. |
| IV. Leverage the Ecosystem | ✅ Pass | All dependencies are established crates (serde, tokio, thiserror, etc.). No hand-rolled alternatives. |
| V. Provider Agnosticism | ✅ Pass | Core has no provider-specific dependencies. Adapters crate is separate. |
| VI. Safety & Correctness | ✅ Pass | `#[forbid(unsafe_code)]` on all library crates. Clippy all+pedantic+nursery as errors. |

No violations. Complexity Tracking not needed.

## Project Structure

### Documentation (this feature)

```text
specs/001-workspace-scaffold/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
└── tasks.md             # Phase 2 output (/speckit.tasks command)
```

### Source Code (repository root)

```text
Cargo.toml                  # Workspace root + core crate [package]
rust-toolchain.toml         # Pins Rust 1.88
rustfmt.toml                # Formatter configuration
src/
└── lib.rs                  # Core crate: #[forbid(unsafe_code)], public re-exports

adapters/
├── Cargo.toml              # swink-agent-adapters, depends on swink-agent
└── src/
    └── lib.rs              # #[forbid(unsafe_code)], stub re-exports

memory/
├── Cargo.toml              # swink-agent-memory, depends on swink-agent
└── src/
    └── lib.rs              # #[forbid(unsafe_code)], stub re-exports

local-llm/
├── Cargo.toml              # swink-agent-local-llm, depends on swink-agent
└── src/
    └── lib.rs              # #[forbid(unsafe_code)], stub re-exports

eval/
├── Cargo.toml              # swink-agent-eval, depends on swink-agent
└── src/
    └── lib.rs              # #[forbid(unsafe_code)], stub re-exports

tui/
├── Cargo.toml              # swink-agent-tui (binary), depends on core + adapters + memory + local-llm
└── src/
    ├── main.rs             # Entry point (minimal)
    └── lib.rs              # #[forbid(unsafe_code)], stub re-exports

xtask/
├── Cargo.toml              # xtask, no production dependencies
└── src/
    └── main.rs             # Empty main()

tests/
└── common/
    └── mod.rs              # Shared test helpers (MockStreamFn, MockTool, etc.)
```

**Structure Decision**: Rust workspace with root `Cargo.toml` defining all
seven members. Each subcrate has its own `Cargo.toml` and `src/` directory.
This matches the standard Cargo workspace layout and the dependency chain
defined in the spec (FR-010).

## Complexity Tracking

> No Constitution Check violations. Table intentionally left empty.
