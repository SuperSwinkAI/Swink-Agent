# Tasks: Context Management

**Input**: Design documents from `/specs/006-context-management/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md, quickstart.md

**Tests**: Inline unit tests per module (Rust convention). Tests are included as this is a test-driven project per CLAUDE.md.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Module scaffolding and shared types needed by all context management components

**Note**: Source files (`context.rs`, `context_transformer.rs`, `async_context_transformer.rs`, `context_version.rs`, `convert.rs`) already exist with partial implementations from prior work. Tasks in this phase verify the modules exist and are correctly registered; subsequent phases extend or complete the existing code.

- [ ] T001 Verify `src/context.rs` exists with `#![forbid(unsafe_code)]`, module-level doc comment, and imports for `AgentMessage`, `LlmMessage`, `ContentBlock`, `serde_json::Value` — add any missing elements
- [ ] T002 Verify `src/context_transformer.rs` exists with `#![forbid(unsafe_code)]`, module-level doc comment, and import of `AgentMessage` — add any missing elements
- [ ] T003 [P] Verify `src/async_context_transformer.rs` exists with `#![forbid(unsafe_code)]`, module-level doc comment, and imports for `AgentMessage`, `Pin`, `Future` — add any missing elements
- [ ] T004 [P] Verify `src/context_version.rs` exists with `#![forbid(unsafe_code)]`, module-level doc comment, and imports for `LlmMessage`, `Mutex` — add any missing elements
- [ ] T005 [P] Verify `src/convert.rs` exists with `#![forbid(unsafe_code)]`, module-level doc comment, and imports for `AgentMessage`, `UserMessage`, `AssistantMessage`, `ToolResultMessage`, `AgentTool`, `serde_json::Value` — add any missing elements
- [ ] T006 Verify all five modules are registered in `src/lib.rs`: `mod context;`, `mod context_transformer;`, `mod async_context_transformer;`, `mod context_version;`, `pub mod convert;` — add any missing registrations

**Checkpoint**: All module files exist and compile (empty). `cargo build -p swink-agent` passes.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Token estimation and `CompactionResult` — used by sliding window (US1), transformer traits (US2), and versioning (US4)

**CRITICAL**: No user story work can begin until this phase is complete

- [ ] T007 Add unit tests for `estimate_tokens()` in `src/context.rs`: test LlmMessage with text blocks, mixed content blocks, empty message, and CustomMessage (100 tokens flat). Also test idempotency (same message → same result)
- [ ] T008 Implement `estimate_tokens()` function in `src/context.rs` — chars/4 heuristic for `LlmMessage` text content blocks, 100 tokens flat for `CustomMessage`
- [ ] T009 Implement `TokenCounter` trait in `src/context.rs` with `fn count_tokens(&self, &AgentMessage) -> usize` (require `Send + Sync`)
- [ ] T010 Implement `DefaultTokenCounter` struct in `src/context.rs` — derives `Debug, Clone, Copy, Default`, delegates to `estimate_tokens()`
- [ ] T011 Implement `CompactionResult` struct in `src/context.rs` with fields `dropped_count: usize`, `tokens_before: usize`, `tokens_after: usize` (derive `Debug, Clone`)
- [ ] T012 Implement `CompactionReport` struct in `src/context_transformer.rs` with fields `dropped_count: usize`, `tokens_before: usize`, `tokens_after: usize`, `overflow: bool` (derive `Debug, Clone`)
- [ ] T013 Add re-exports to `src/lib.rs`: `pub use context::{estimate_tokens, TokenCounter, DefaultTokenCounter};` and `pub use context_transformer::CompactionReport;`

**Checkpoint**: Foundation ready — `cargo test -p swink-agent context` passes. Token estimation and result types available for all user stories.

---

## Phase 3: User Story 1 — Automatic Context Pruning for Long Conversations (Priority: P1)

**Goal**: Sliding window algorithm that preserves anchor messages (first N) and tail messages (most recent), removes the middle to fit a token budget, and keeps tool call/result pairs together.

**Independent Test**: Create a message history exceeding a token budget, apply `compact_sliding_window()`, verify anchor and tail preserved, middle removed, tool-result pairs intact.

### Implementation for User Story 1

- [ ] T014 [US1] Add unit tests for sliding window in `src/context.rs`: (a) messages within budget — no compaction, (b) messages exceeding budget — anchor + tail preserved, middle removed, (c) tool-result pair kept together even if over budget, (d) overflow flag selects overflow_budget, (e) empty message list — no-op
- [ ] T015 [US1] Implement `compact_sliding_window()` in `src/context.rs` — core algorithm: preserve `anchor` messages, walk backward from end accumulating tail messages within `budget - anchor_tokens`, remove contiguous middle section. Return `Option<CompactionResult>` (None if no compaction needed)
- [ ] T016 [US1] Implement tool-result pair preservation in `compact_sliding_window()` — when a `ToolResult` message is included in the tail, the preceding `Assistant` tool-call message must also be included even if it exceeds the budget
- [ ] T017 [US1] Implement `compact_sliding_window_with()` in `src/context.rs` — variant accepting `Option<&dyn TokenCounter>` for pluggable token counting (falls back to `DefaultTokenCounter`)
- [ ] T018 [US1] Implement `sliding_window()` factory function in `src/context.rs` — returns `impl Fn(&mut Vec<AgentMessage>, bool) + Send + Sync` closure that selects `normal_budget` or `overflow_budget` based on the `bool` overflow flag, delegates to `compact_sliding_window()`
- [ ] T019 [US1] Add re-exports to `src/lib.rs`: `pub use context::sliding_window;`

**Checkpoint**: `cargo test -p swink-agent context` passes. Sliding window algorithm works standalone with default and custom token counters.

---

## Phase 4: User Story 2 — Custom Context Transformation (Priority: P1)

**Goal**: Pluggable synchronous and asynchronous transformation hooks that run before each LLM call, with overflow signal propagation.

**Independent Test**: Provide a transformation hook that modifies the message list and verify the modified context is returned. Verify overflow signal is received on retry.

### Implementation for User Story 2

- [ ] T020 [US2] Add unit tests for `ContextTransformer` in `src/context_transformer.rs`: (a) closure-based transformer called with overflow=false, (b) closure-based transformer called with overflow=true, (c) `SlidingWindowTransformer` compacts and returns report, (d) `SlidingWindowTransformer` with custom `TokenCounter`
- [ ] T021 [US2] Add unit tests for `AsyncContextTransformer` in `src/async_context_transformer.rs`: (a) async transformer called and awaited, (b) overflow signal propagated to async transformer
- [ ] T022 [US2] Implement `ContextTransformer` trait in `src/context_transformer.rs` — `fn transform(&self, &mut Vec<AgentMessage>, bool) -> Option<CompactionReport>` (require `Send + Sync`)
- [ ] T023 [US2] Implement blanket impl of `ContextTransformer` for closures in `src/context_transformer.rs` — `impl<F: Fn(&mut Vec<AgentMessage>, bool) + Send + Sync> ContextTransformer for F` (returns `None` from `transform`)
- [ ] T024 [US2] Implement `SlidingWindowTransformer` struct in `src/context_transformer.rs` — fields: `normal_budget`, `overflow_budget`, `anchor`, `token_counter: Option<Arc<dyn TokenCounter>>`. Constructor `new()`, builder method `with_token_counter()`. Implements `ContextTransformer` delegating to `compact_sliding_window_with()` and returning `CompactionReport`
- [ ] T025 [US2] Implement `AsyncContextTransformer` trait in `src/async_context_transformer.rs` — `fn transform<'a>(&'a self, &'a mut Vec<AgentMessage>, bool) -> Pin<Box<dyn Future<Output = Option<CompactionReport>> + Send + 'a>>`
- [ ] T026 [US2] Add re-exports to `src/lib.rs`: `pub use context_transformer::{ContextTransformer, SlidingWindowTransformer};` and `pub use async_context_transformer::AsyncContextTransformer;`

**Checkpoint**: `cargo test -p swink-agent context_transformer` and `cargo test -p swink-agent async_context` pass. Both sync and async transformer traits work with struct and closure implementations.

---

## Phase 5: User Story 3 — Message Conversion Pipeline (Priority: P1)

**Goal**: Provider-agnostic message conversion that maps agent messages to provider format, filtering out custom messages.

**Independent Test**: Create a message history with standard and custom messages, apply `convert_messages()`, verify custom messages filtered out and standard messages converted.

### Implementation for User Story 3

- [ ] T027 [US3] Add unit tests for message conversion in `src/convert.rs`: (a) standard messages converted via mock `MessageConverter`, (b) `CustomMessage` variants filtered out, (c) system message prepended when converter returns `Some`, (d) system message omitted when converter returns `None`, (e) `extract_tool_schemas()` extracts correct schemas
- [ ] T028 [US3] Implement `MessageConverter` trait in `src/convert.rs` — associated type `Message`, methods: `system_message(&str) -> Option<Self::Message>`, `user_message(&UserMessage) -> Self::Message`, `assistant_message(&AssistantMessage) -> Self::Message`, `tool_result_message(&ToolResultMessage) -> Self::Message`
- [ ] T029 [US3] Implement `convert_messages<C: MessageConverter>()` generic function in `src/convert.rs` — iterate agent messages, match on `LlmMessage` variants (User/Assistant/ToolResult), skip `CustomMessage`, prepend system message if `system_message()` returns `Some`
- [ ] T030 [US3] Implement `ToolSchema` struct in `src/convert.rs` with fields `name: String`, `description: String`, `parameters: Value` (derive `Debug, Clone`)
- [ ] T031 [US3] Implement `extract_tool_schemas()` function in `src/convert.rs` — iterate `&[Arc<dyn AgentTool>]`, extract name, description, and JSON schema parameters from each tool
- [ ] T032 [US3] Add re-exports to `src/lib.rs`: `pub use convert::{MessageConverter, convert_messages, ToolSchema, extract_tool_schemas};`

**Checkpoint**: `cargo test -p swink-agent convert` passes. Message conversion works with a mock converter, custom messages filtered.

---

## Phase 6: User Story 4 — Versioned Context History (Priority: P3)

**Goal**: Track how context evolves across turns by capturing dropped messages as versioned snapshots during compaction.

**Independent Test**: Run a multi-turn compaction, verify each turn's dropped messages are retrievable as distinct versions with correct metadata.

### Implementation for User Story 4

- [ ] T033 [US4] Add unit tests for context versioning in `src/context_version.rs`: (a) `InMemoryVersionStore` save/load/list round-trip, `len()` and `is_empty()`, (b) `latest_version()` returns most recent, (c) `VersioningTransformer` captures dropped messages as version, (d) version numbers are monotonically increasing, (e) summarizer called when provided, (f) no version created when no compaction occurs
- [ ] T034 [US4] Implement `ContextVersion` struct in `src/context_version.rs` with fields: `version: u64`, `turn: u64`, `timestamp: u64`, `messages: Vec<LlmMessage>`, `summary: Option<String>` (derive `Debug, Clone`)
- [ ] T035 [US4] Implement `ContextVersionMeta` struct in `src/context_version.rs` with fields: `version: u64`, `turn: u64`, `timestamp: u64`, `message_count: usize`, `has_summary: bool` (derive `Debug, Clone`)
- [ ] T036 [US4] Implement `ContextVersionStore` trait in `src/context_version.rs` — methods: `save_version(&self, &ContextVersion)`, `load_version(&self, u64) -> Option<ContextVersion>`, `list_versions(&self) -> Vec<ContextVersionMeta>`, `latest_version(&self) -> Option<ContextVersion>` (default impl). Require `Send + Sync`
- [ ] T037 [US4] Implement `InMemoryVersionStore` in `src/context_version.rs` — `Mutex<Vec<ContextVersion>>` with poison recovery via `into_inner()`. Methods: `new()` (const fn), `len()`, `is_empty()`. Implements `ContextVersionStore`
- [ ] T038 [US4] Implement `ContextSummarizer` trait in `src/context_version.rs` — `fn summarize(&self, &[LlmMessage]) -> Option<String>` (require `Send + Sync`)
- [ ] T039 [US4] Implement `VersioningTransformer` struct in `src/context_version.rs` — wraps `Box<dyn ContextTransformer>`, holds `Arc<dyn ContextVersionStore>`, optional `Arc<dyn ContextSummarizer>`, internal `Mutex<VersioningState>` (next_version, turn_counter). Constructor `new()`, builder `with_summarizer()`, accessor `store()`
- [ ] T040 [US4] Implement `ContextTransformer` for `VersioningTransformer` in `src/context_version.rs` — snapshot messages before inner transform, diff after to find dropped messages, save as `ContextVersion` with monotonic version number and Unix timestamp. Optionally summarize via `ContextSummarizer`
- [ ] T041 [US4] Add re-exports to `src/lib.rs`: `pub use context_version::{ContextVersion, ContextVersionMeta, ContextVersionStore, InMemoryVersionStore, ContextSummarizer, VersioningTransformer};`

**Checkpoint**: `cargo test -p swink-agent context_version` passes. Versioning captures dropped messages, versions retrievable, summarizer integration works.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Integration, API surface completeness, and validation across all stories

- [ ] T042 Verify all public types are re-exported from `src/lib.rs` per contracts/public-api.md — cross-reference every type/function listed in the contract
- [ ] T043 Run `cargo clippy --workspace -- -D warnings` and fix any warnings in context management modules
- [ ] T044 Run `cargo test --workspace` to verify no regressions across the full workspace
- [ ] T045 Run `cargo test -p swink-agent --no-default-features` to verify context management works with builtin-tools disabled
- [ ] T046 Validate quickstart.md code examples compile by inspection — check `sliding_window`, `SlidingWindowTransformer`, `VersioningTransformer`, `convert_messages` usage patterns match the public API

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Story 1 (Phase 3)**: Depends on Foundational (Phase 2) — sliding window uses `estimate_tokens`, `TokenCounter`, `CompactionResult`
- **User Story 2 (Phase 4)**: Depends on Foundational (Phase 2) and User Story 1 (Phase 3) — `SlidingWindowTransformer` delegates to `compact_sliding_window_with()`
- **User Story 3 (Phase 5)**: Depends on Foundational (Phase 2) only — message conversion is independent of sliding window and transformers
- **User Story 4 (Phase 6)**: Depends on Foundational (Phase 2) and User Story 2 (Phase 4) — `VersioningTransformer` wraps `ContextTransformer`
- **Polish (Phase 7)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (Sliding Window)**: After Foundational — no story dependencies
- **US2 (Transformers)**: After US1 — `SlidingWindowTransformer` wraps `compact_sliding_window_with()`
- **US3 (Message Conversion)**: After Foundational — independent of US1/US2, can run in parallel with them
- **US4 (Versioning)**: After US2 — `VersioningTransformer` wraps `ContextTransformer`

### Within Each User Story

- Types/structs before algorithms
- Core implementation before builder methods
- Tests before implementation (Constitution II: TDD — red-green-refactor)
- Re-exports after implementation

### Parallel Opportunities

- T003, T004, T005 can run in parallel (independent module files)
- US1 and US3 can run in parallel after Foundational (different files, no dependencies)
- Within US1: T014 and T015 are sequential (same function), but T016 and T017 can follow immediately
- Within US3: T027, T029 can run in parallel (different types in same file)
- Within US4: T033, T034 can run in parallel (independent structs)

---

## Parallel Example: User Story 1

```
# After Foundational phase is complete:

# Tests first (TDD red phase):
Task T014: Unit tests for sliding window in src/context.rs

# Sequential implementation (green phase, building on each other):
Task T015: Core sliding window algorithm in src/context.rs
Task T016: Tool-result pair preservation in src/context.rs
Task T017: compact_sliding_window_with() variant in src/context.rs
Task T018: sliding_window() factory function in src/context.rs
Task T019: Re-exports in src/lib.rs
```

## Parallel Example: User Story 3 (can run alongside US1)

```
# After Foundational phase is complete:

# Tests first (TDD red phase):
Task T027: Unit tests for message conversion in src/convert.rs

# Implementation (green phase):
Task T028: MessageConverter trait in src/convert.rs
Task T029: convert_messages() in src/convert.rs
Task T030: ToolSchema struct in src/convert.rs
Task T031: extract_tool_schemas() in src/convert.rs
Task T032: Re-exports in src/lib.rs
```

---

## Implementation Strategy

### MVP First (User Story 1 + User Story 3)

1. Complete Phase 1: Setup (module scaffolding)
2. Complete Phase 2: Foundational (token estimation, result types)
3. Complete Phase 3: User Story 1 (sliding window)
4. Complete Phase 5: User Story 3 (message conversion) — can run in parallel with US1
5. **STOP and VALIDATE**: `cargo test -p swink-agent context` and `cargo test -p swink-agent convert` both pass
6. Core context management is functional

### Incremental Delivery

1. Setup + Foundational -> Foundation ready
2. Add US1 (Sliding Window) -> Test independently -> Core pruning works
3. Add US3 (Message Conversion) -> Test independently -> Provider pipeline works
4. Add US2 (Transformers) -> Test independently -> Pluggable transform hooks work
5. Add US4 (Versioning) -> Test independently -> Debug/observability layer works
6. Polish -> Full workspace validation

### Parallel Team Strategy

With multiple developers:

1. Team completes Setup + Foundational together
2. Once Foundational is done:
   - Developer A: US1 (Sliding Window) then US2 (Transformers)
   - Developer B: US3 (Message Conversion) then US4 (Versioning, after US2 merges)
3. Stories complete and integrate independently
