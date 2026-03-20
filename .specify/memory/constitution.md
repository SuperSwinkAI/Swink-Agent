<!--
Sync Impact Report
- Version change: (none) → 1.0.0
- Added principles: I. Library-First, II. Test-Driven Development,
  III. Efficiency & Performance, IV. Leverage the Ecosystem,
  V. Provider Agnosticism, VI. Safety & Correctness
- Added sections: Architectural Constraints, Development Workflow
- Templates requiring updates:
  - .specify/templates/plan-template.md — ✅ no changes needed
    (Constitution Check section already references constitution file)
  - .specify/templates/spec-template.md — ✅ no changes needed
  - .specify/templates/tasks-template.md — ✅ no changes needed
- Follow-up TODOs: none
-->

# Swink Agent Constitution

## Core Principles

### I. Library-First

The harness is a library crate, not a service or daemon. Every feature
starts as part of a crate's public API surface. Crates MUST be
self-contained, independently compilable, and independently testable.
Workspace members follow the defined dependency chain — no reverse
dependencies, no circular imports. The core crate (`swink-agent`) MUST
remain free of provider-specific, UI-specific, or storage-specific
dependencies. New functionality slots into the existing crate that owns
that concern; a new crate is justified only when the dependency would
violate an existing boundary.

### II. Test-Driven Development (NON-NEGOTIABLE)

Red-Green-Refactor is the enforced development cycle. Tests MUST be
written before the implementation they verify. A bug report produces a
failing regression test before the fix. `cargo test --workspace` MUST
pass before every commit. Tests are descriptive `snake_case` without a
`test_` prefix. Shared helpers live in `tests/common/mod.rs`. Mocks are
prefixed `Mock`. Integration tests that exercise real I/O (network,
filesystem) are preferred over mocks when feasible — mocks are used only
when the real dependency is unavailable, slow, or non-deterministic.

### III. Efficiency & Performance

Minimize allocations on hot paths. Use `tokio::spawn` for concurrent
tool execution, not sequential awaits. Prefer zero-copy and borrowed
data where the borrow checker allows. Profile before optimizing —
measure, don't guess. Build profiles MUST use `split-debuginfo` for dev
and `lto = "thin"` with `codegen-units = 1` for release. Token
estimation uses a chars/4 heuristic — keep it simple until profiling
shows otherwise.

### IV. Leverage the Ecosystem

Prefer well-maintained, widely-used public crates over hand-rolled
implementations. Rebuilding what already exists as a high-quality library
wastes time and introduces maintenance burden. Workspace dependency
versions MUST be centralized in the root `Cargo.toml` so all crates
resolve common libraries to the same version. When evaluating a
dependency: check download count, maintenance activity, and API
stability. If a crate does 80% of what is needed, wrap it — do not
rewrite it.

### V. Provider Agnosticism

`StreamFn` is the sole provider boundary. The core crate MUST never hold
an API key, SDK client, or provider-specific type. All LLM communication
flows through the `StreamFn` trait — direct providers, proxies, mocks,
and local models all satisfy the same interface. Adding a new provider
means adding a module to the adapters crate with zero changes to core.

### VI. Safety & Correctness

`#[forbid(unsafe_code)]` at every crate root — no exceptions.
`clippy::all`, `clippy::pedantic`, and `clippy::nursery` warnings are
errors. LLM and tool errors produce assistant messages with
`stop_reason: error`, never panics. Errors stay in the message log so
callers always get a complete, inspectable history. Event subscriber
panics are caught via `catch_unwind` and the panicking subscriber is
auto-removed. Poisoned mutexes recover via `into_inner()` — never panic
on lock acquisition.

## Architectural Constraints

- **Crate count**: Seven workspace members (core, adapters, memory,
  local-llm, eval, TUI, xtask). Adding a crate requires justification
  that no existing crate boundary can absorb the concern.
- **MSRV**: 1.88, edition 2024. Pinned via `rust-toolchain.toml`.
- **Concurrency model**: Tool calls within a turn run concurrently via
  `tokio::spawn`. Everything else (turns, steering polls, follow-up
  polls) is sequential.
- **Events are outward-only**: The event system pushes to subscribers.
  Hooks that mutate execution are callbacks in `AgentLoopConfig`, not
  event responses. No re-entrant state.
- **No global mutable state**: All shared state uses `Arc<Mutex<>>` or
  message passing.

## Development Workflow

- **Import order**: `std` → external (alphabetical) → `crate::`/`super::`.
- **Naming**: `new()` primary constructor; `with_*()` builder chain.
  No `get_` prefix on getters. `is_*`/`has_*` for predicates. Closure
  type aliases suffixed `Fn`. Trailing `_` for reserved-word modules.
- **File size**: One concern per file. Split at ~1500 lines.
- **Re-exports**: `lib.rs` re-exports the public API — consumers never
  reach into submodules.
- **Formatting**: `rustfmt` with project configuration. Deterministic
  output across environments.
- **Lessons learned**: Non-obvious discoveries go in nested `CLAUDE.md`
  files. Update when you find something surprising.

## Governance

This constitution is the authoritative source of project principles.
All code reviews and feature plans MUST verify compliance. Amendments
require: (1) a description of the change and its rationale,
(2) an updated version number following semver, and (3) a propagation
check across dependent templates and documentation.

Complexity beyond what the constitution permits MUST be justified in
the plan's Complexity Tracking table with a rejected simpler
alternative.

**Version**: 1.0.0 | **Ratified**: 2026-03-20 | **Last Amended**: 2026-03-20
