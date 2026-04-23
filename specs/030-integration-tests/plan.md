# Implementation Plan: Integration Tests

**Branch**: `030-integration-tests` | **Date**: 2026-03-20 | **Spec**: [spec.md](./spec.md)
**Input**: Feature specification from `/specs/030-integration-tests/spec.md`

## Summary

Build a comprehensive integration test suite exercising the full Agent → loop → mock stream → tool execution → events stack. Each PRD acceptance criterion (AC 1–30) maps to at least one integration test. All tests use shared mock infrastructure (`MockStreamFn`, `MockTool`, `EventCollector`, helpers) from `tests/common/mod.rs` and run without external services, network access, or API keys. Tests are organized into six files matching the spec's user story groupings.

## Technical Context

**Language/Version**: Rust latest stable, edition 2024
**Primary Dependencies**: `swink-agent` (core), `swink-agent-adapters` (proxy), `swink-agent-tui` (headless rendering), `tokio` (async runtime), `serde_json` (mock data), `futures` (stream combinators)
**Storage**: N/A — all state is in-memory mocks
**Testing**: `cargo test --workspace` — workspace-level integration tests in `tests/` directory
**Target Platform**: All platforms (no platform-specific I/O)
**Project Type**: Library test suite
**Performance Goals**: Each individual test completes in under 10 seconds; full suite under CI timeout
**Constraints**: Zero external dependencies — no network, no API keys, no filesystem side effects
**Scale/Scope**: 30+ integration tests covering AC 1–30, plus edge case tests

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| # | Principle | Status | Notes |
|---|-----------|--------|-------|
| I | Library-First | PASS | Tests exercise the library's public API only. No service or daemon dependencies. Test files live in the standard `tests/` directory alongside existing integration tests. |
| II | Test-Driven Development | PASS | This feature *is* the test suite. Tests verify already-implemented behavior from specs 001–029. Shared helpers in `tests/common/mod.rs`. Mocks prefixed `Mock`. Descriptive `snake_case` names without `test_` prefix. |
| III | Efficiency & Performance | PASS | Tests use `tokio::spawn` for concurrent tool execution assertions. Timing-independent assertions (event ordering, `Instant` comparisons) avoid slow wall-clock waits. No unnecessary allocations in mock infrastructure. |
| IV | Leverage the Ecosystem | PASS | Uses existing crates: `tokio` (async runtime), `futures` (stream utilities), `serde_json` (mock data), `jsonschema` (schema validation). No new dependencies required. |
| V | Provider Agnosticism | PASS | All tests use `MockStreamFn` implementing the `StreamFn` trait. No provider-specific code. Validates that the `StreamFn` boundary works correctly. |
| VI | Safety & Correctness | PASS | No unsafe code. Tests verify error handling paths (panicking subscribers auto-removed, poisoned mutexes recovered, tool errors surfaced as messages). Tests validate `catch_unwind` behavior. |

## Project Structure

### Documentation (this feature)

```text
specs/030-integration-tests/
├── plan.md              # This file
├── research.md          # Design decisions and alternatives
├── data-model.md        # Entity definitions and mappings
├── contracts/
│   └── public-api.md    # Test helper API contracts
└── quickstart.md        # Getting started guide
```

### Source Code (repository root)

```text
tests/
├── common/
│   └── mod.rs              # Shared mocks: MockStreamFn, MockTool, FlagStreamFn,
│                           # ContextCapturingStreamFn, ApiKeyCapturingStreamFn,
│                           # helpers (text_only_events, tool_call_events, user_msg, etc.)
│
├── integration.rs          # Existing integration tests (6.1–6.15)
│
│  ── New test files (one per user story group) ──
├── ac_lifecycle.rs         # AC 1–5: Agent lifecycle and events
├── ac_tools.rs             # AC 6–12: Tool execution and validation
├── ac_context.rs           # AC 13–16: Context management and overflow
├── ac_resilience.rs        # AC 17–22: Retry, steering, abort
├── ac_structured.rs        # AC 23–25: Structured output, proxy reconstruction
└── ac_tui.rs               # AC 26–30: TUI rendering and interaction
```

**Structure Decision**: Tests are organized by PRD acceptance criteria groups, matching the spec's six user stories. Each file focuses on a cohesive set of related behaviors. All files share the `tests/common/mod.rs` module. This mirrors the existing `tests/integration.rs` pattern already established in the codebase.

## Complexity Tracking

No constitution violations. The test suite adds only test files and extends existing shared helpers — no new crates, no new dependencies, no architectural changes.
