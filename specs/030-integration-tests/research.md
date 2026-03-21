# Research: Integration Tests

**Branch**: `030-integration-tests` | **Date**: 2026-03-20

## Design Decisions

### D1: Test Organization by PRD Acceptance Criteria

**Decision**: Organize integration tests into six files, one per spec user story, each covering a contiguous range of acceptance criteria.

**Rationale**: The PRD defines 30 acceptance criteria grouped into logical clusters (lifecycle, tools, context, resilience, structured output, TUI). Mirroring this grouping in test files makes it trivial to verify coverage: each AC maps to a named test in a predictable file. Developers adding new ACs know exactly where the test belongs.

**Alternatives considered**:
- **Single file**: The existing `integration.rs` already has 15 tests. Adding 30+ more would exceed the ~1500-line guideline and make navigation difficult.
- **One file per AC**: Too granular — 30 files with 1–3 tests each creates unnecessary filesystem noise and `mod common;` boilerplate.

### D2: Shared Mock Infrastructure in `tests/common/mod.rs`

**Decision**: Extend the existing `tests/common/mod.rs` with an `EventCollector` and additional helper functions. Do not create a separate test-support crate.

**Rationale**: The common module already contains `MockStreamFn`, `MockTool`, `FlagStreamFn`, `ContextCapturingStreamFn`, and helper functions (`text_only_events`, `tool_call_events`, `user_msg`). Adding `EventCollector` and new helpers keeps all test infrastructure in one place. A separate crate is unnecessary since these types are only used in `tests/`.

**Alternatives considered**:
- **`test-helpers` feature flag**: The crate already has a `test-helpers` feature but it's for exposing internals to downstream crates, not for organizing test code. Using it would blur the boundary.
- **Inline mocks per test file**: Violates DRY and makes SC-004 (80% helper reuse) impossible.

### D3: Timing-Independent Concurrency Assertions

**Decision**: Assert concurrent tool execution using `Instant` timestamps captured at execution start, not wall-clock duration measurements.

**Rationale**: Wall-clock assertions (e.g., "3 tools with 100ms delay finish in <200ms") are flaky on CI where CPU contention varies. Instead, each mock tool records its start `Instant`. Tests assert that start times of concurrent tools are within a small epsilon of each other, proving they were dispatched in parallel rather than sequentially.

**Alternatives considered**:
- **Sleep-based timing**: Flaky on CI, slow in general.
- **Barrier synchronization**: Overcomplicates mock tools. `Instant`-based assertions are simpler and sufficient.

### D4: Headless TUI Testing

**Decision**: Test TUI components by rendering to an in-memory buffer and asserting on the rendered content. No live terminal required.

**Rationale**: `ratatui` supports rendering to a `TestBackend` that captures widget output as styled strings. Tests can assert on text content, color attributes, and layout without spawning a terminal. This satisfies FR-017 through FR-020 and runs in CI without a TTY.

**Alternatives considered**:
- **Screenshot comparison**: Brittle across platforms, hard to maintain.
- **Skip TUI tests in CI**: Defeats the purpose of AC 26–30.

### D5: EventCollector Design

**Decision**: Implement `EventCollector` as a struct wrapping `Arc<Mutex<Vec<AgentEvent>>>` that can be registered as an event subscriber. Provide filtering methods (`events_of_type`, `turn_starts`, `turn_ends`) for convenient assertions.

**Rationale**: The agent's event system pushes events to subscribers via callbacks. `EventCollector` captures these into a shared vector that tests inspect after the agent turn completes. The `Arc<Mutex<>>` pattern matches the project's existing concurrency approach (constitution: "No global mutable state: All shared state uses `Arc<Mutex<>>` or message passing").

**Alternatives considered**:
- **Channel-based collection**: Adds complexity (receiver must be drained) without benefit for synchronous post-hoc assertions.
- **Raw `Vec` without `Arc`**: Cannot be shared between the subscriber closure and the test assertion code.
