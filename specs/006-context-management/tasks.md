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

- [x] T001 Verify `src/context.rs` exists with `#![forbid(unsafe_code)]`, module-level doc comment, and imports for `AgentMessage`, `LlmMessage`, `ContentBlock`, `serde_json::Value` — add any missing elements
- [x] T002 Verify `src/context_transformer.rs` exists with `#![forbid(unsafe_code)]`, module-level doc comment, and import of `AgentMessage` — add any missing elements
- [x] T003 [P] Verify `src/async_context_transformer.rs` exists with `#![forbid(unsafe_code)]`, module-level doc comment, and imports for `AgentMessage`, `Pin`, `Future` — add any missing elements
- [x] T004 [P] Verify `src/context_version.rs` exists with `#![forbid(unsafe_code)]`, module-level doc comment, and imports for `LlmMessage`, `Mutex` — add any missing elements
- [x] T005 [P] Verify `src/convert.rs` exists with `#![forbid(unsafe_code)]`, module-level doc comment, and imports for `AgentMessage`, `UserMessage`, `AssistantMessage`, `ToolResultMessage`, `AgentTool`, `serde_json::Value` — add any missing elements
- [x] T006 Verify all five modules are registered in `src/lib.rs`: `mod context;`, `mod context_transformer;`, `mod async_context_transformer;`, `mod context_version;`, `pub mod convert;` — add any missing registrations

**Checkpoint**: All module files exist and compile (empty). `cargo build -p swink-agent` passes.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Token estimation and `CompactionResult` — used by sliding window (US1), transformer traits (US2), and versioning (US4)

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T007 Add unit tests for `estimate_tokens()` in `src/context.rs`: test LlmMessage with text blocks, mixed content blocks, empty message, and CustomMessage (100 tokens flat). Also test idempotency (same message → same result)
- [x] T008 Implement `estimate_tokens()` function in `src/context.rs` — chars/4 heuristic for `LlmMessage` text content blocks, 100 tokens flat for `CustomMessage`
- [x] T009 Implement `TokenCounter` trait in `src/context.rs` with `fn count_tokens(&self, &AgentMessage) -> usize` (require `Send + Sync`)
- [x] T010 Implement `DefaultTokenCounter` struct in `src/context.rs` — derives `Debug, Clone, Copy, Default`, delegates to `estimate_tokens()`
- [x] T011 Implement `CompactionResult` struct in `src/context.rs` with fields `dropped_count: usize`, `tokens_before: usize`, `tokens_after: usize` (derive `Debug, Clone`)
- [x] T012 Implement `CompactionReport` struct in `src/context_transformer.rs` with fields `dropped_count: usize`, `tokens_before: usize`, `tokens_after: usize`, `overflow: bool` (derive `Debug, Clone`)
- [x] T013 Add re-exports to `src/lib.rs`: `pub use context::{estimate_tokens, TokenCounter, DefaultTokenCounter};` and `pub use context_transformer::CompactionReport;`

**Checkpoint**: Foundation ready — `cargo test -p swink-agent context` passes. Token estimation and result types available for all user stories.

---

## Phase 3: User Story 1 — Automatic Context Pruning for Long Conversations (Priority: P1)

**Goal**: Sliding window algorithm that preserves anchor messages (first N) and tail messages (most recent), removes the middle to fit a token budget, and keeps tool call/result pairs together.

**Independent Test**: Create a message history exceeding a token budget, apply `compact_sliding_window()`, verify anchor and tail preserved, middle removed, tool-result pairs intact.

### Implementation for User Story 1

- [x] T014 [US1] Add unit tests for sliding window in `src/context.rs`: (a) messages within budget — no compaction, (b) messages exceeding budget — anchor + tail preserved, middle removed, (c) tool-result pair kept together even if over budget, (d) overflow flag selects overflow_budget, (e) empty message list — no-op
- [x] T015 [US1] Implement `compact_sliding_window()` in `src/context.rs` — core algorithm: preserve `anchor` messages, walk backward from end accumulating tail messages within `budget - anchor_tokens`, remove contiguous middle section. Return `Option<CompactionResult>` (None if no compaction needed)
- [x] T016 [US1] Implement tool-result pair preservation in `compact_sliding_window()` — when a `ToolResult` message is included in the tail, the preceding `Assistant` tool-call message must also be included even if it exceeds the budget
- [x] T017 [US1] Implement `compact_sliding_window_with()` in `src/context.rs` — variant accepting `Option<&dyn TokenCounter>` for pluggable token counting (falls back to `DefaultTokenCounter`)
- [x] T018 [US1] Implement `sliding_window()` factory function in `src/context.rs` — returns `impl Fn(&mut Vec<AgentMessage>, bool) + Send + Sync` closure that selects `normal_budget` or `overflow_budget` based on the `bool` overflow flag, delegates to `compact_sliding_window()`
- [x] T019 [US1] Add re-exports to `src/lib.rs`: `pub use context::sliding_window;`

**Checkpoint**: `cargo test -p swink-agent context` passes. Sliding window algorithm works standalone with default and custom token counters.

---

## Phase 4: User Story 2 — Custom Context Transformation (Priority: P1)

**Goal**: Pluggable synchronous and asynchronous transformation hooks that run before each LLM call, with overflow signal propagation.

**Independent Test**: Provide a transformation hook that modifies the message list and verify the modified context is returned. Verify overflow signal is received on retry.

### Implementation for User Story 2

- [x] T020 [US2] Add unit tests for `ContextTransformer` in `src/context_transformer.rs`: (a) closure-based transformer called with overflow=false, (b) closure-based transformer called with overflow=true, (c) `SlidingWindowTransformer` compacts and returns report, (d) `SlidingWindowTransformer` with custom `TokenCounter`
- [x] T021 [US2] Add unit tests for `AsyncContextTransformer` in `src/async_context_transformer.rs`: (a) async transformer called and awaited, (b) overflow signal propagated to async transformer
- [x] T022 [US2] Implement `ContextTransformer` trait in `src/context_transformer.rs` — `fn transform(&self, &mut Vec<AgentMessage>, bool) -> Option<CompactionReport>` (require `Send + Sync`)
- [x] T023 [US2] Implement blanket impl of `ContextTransformer` for closures in `src/context_transformer.rs` — `impl<F: Fn(&mut Vec<AgentMessage>, bool) + Send + Sync> ContextTransformer for F` (returns `None` from `transform`)
- [x] T024 [US2] Implement `SlidingWindowTransformer` struct in `src/context_transformer.rs` — fields: `normal_budget`, `overflow_budget`, `anchor`, `token_counter: Option<Arc<dyn TokenCounter>>`. Constructor `new()`, builder method `with_token_counter()`. Implements `ContextTransformer` delegating to `compact_sliding_window_with()` and returning `CompactionReport`
- [x] T025 [US2] Implement `AsyncContextTransformer` trait in `src/async_context_transformer.rs` — `fn transform<'a>(&'a self, &'a mut Vec<AgentMessage>, bool) -> Pin<Box<dyn Future<Output = Option<CompactionReport>> + Send + 'a>>`
- [x] T026 [US2] Add re-exports to `src/lib.rs`: `pub use context_transformer::{ContextTransformer, SlidingWindowTransformer};` and `pub use async_context_transformer::AsyncContextTransformer;`

**Checkpoint**: `cargo test -p swink-agent context_transformer` and `cargo test -p swink-agent async_context` pass. Both sync and async transformer traits work with struct and closure implementations.

---

## Phase 5: User Story 3 — Message Conversion Pipeline (Priority: P1)

**Goal**: Provider-agnostic message conversion that maps agent messages to provider format, filtering out custom messages.

**Independent Test**: Create a message history with standard and custom messages, apply `convert_messages()`, verify custom messages filtered out and standard messages converted.

### Implementation for User Story 3

- [x] T027 [US3] Add unit tests for message conversion in `src/convert.rs`: (a) standard messages converted via mock `MessageConverter`, (b) `CustomMessage` variants filtered out, (c) system message prepended when converter returns `Some`, (d) system message omitted when converter returns `None`, (e) `extract_tool_schemas()` extracts correct schemas
- [x] T028 [US3] Implement `MessageConverter` trait in `src/convert.rs` — associated type `Message`, methods: `system_message(&str) -> Option<Self::Message>`, `user_message(&UserMessage) -> Self::Message`, `assistant_message(&AssistantMessage) -> Self::Message`, `tool_result_message(&ToolResultMessage) -> Self::Message`
- [x] T029 [US3] Implement `convert_messages<C: MessageConverter>()` generic function in `src/convert.rs` — iterate agent messages, match on `LlmMessage` variants (User/Assistant/ToolResult), skip `CustomMessage`, prepend system message if `system_message()` returns `Some`
- [x] T030 [US3] Implement `ToolSchema` struct in `src/convert.rs` with fields `name: String`, `description: String`, `parameters: Value` (derive `Debug, Clone`)
- [x] T031 [US3] Implement `extract_tool_schemas()` function in `src/convert.rs` — iterate `&[Arc<dyn AgentTool>]`, extract name, description, and JSON schema parameters from each tool
- [x] T032 [US3] Add re-exports to `src/lib.rs`: `pub use convert::{MessageConverter, convert_messages, ToolSchema, extract_tool_schemas};`

**Checkpoint**: `cargo test -p swink-agent convert` passes. Message conversion works with a mock converter, custom messages filtered.

---

## Phase 6: User Story 4 — Versioned Context History (Priority: P3)

**Goal**: Track how context evolves across turns by capturing dropped messages as versioned snapshots during compaction.

**Independent Test**: Run a multi-turn compaction, verify each turn's dropped messages are retrievable as distinct versions with correct metadata.

### Implementation for User Story 4

- [x] T033 [US4] Add unit tests for context versioning in `src/context_version.rs`: (a) `InMemoryVersionStore` save/load/list round-trip, `len()` and `is_empty()`, (b) `latest_version()` returns most recent, (c) `VersioningTransformer` captures dropped messages as version, (d) version numbers are monotonically increasing, (e) summarizer called when provided, (f) no version created when no compaction occurs
- [x] T034 [US4] Implement `ContextVersion` struct in `src/context_version.rs` with fields: `version: u64`, `turn: u64`, `timestamp: u64`, `messages: Vec<LlmMessage>`, `summary: Option<String>` (derive `Debug, Clone`)
- [x] T035 [US4] Implement `ContextVersionMeta` struct in `src/context_version.rs` with fields: `version: u64`, `turn: u64`, `timestamp: u64`, `message_count: usize`, `has_summary: bool` (derive `Debug, Clone`)
- [x] T036 [US4] Implement `ContextVersionStore` trait in `src/context_version.rs` — methods: `save_version(&self, &ContextVersion)`, `load_version(&self, u64) -> Option<ContextVersion>`, `list_versions(&self) -> Vec<ContextVersionMeta>`, `latest_version(&self) -> Option<ContextVersion>` (default impl). Require `Send + Sync`
- [x] T037 [US4] Implement `InMemoryVersionStore` in `src/context_version.rs` — `Mutex<Vec<ContextVersion>>` with poison recovery via `into_inner()`. Methods: `new()` (const fn), `len()`, `is_empty()`. Implements `ContextVersionStore`
- [x] T038 [US4] Implement `ContextSummarizer` trait in `src/context_version.rs` — `fn summarize(&self, &[LlmMessage]) -> Option<String>` (require `Send + Sync`)
- [x] T039 [US4] Implement `VersioningTransformer` struct in `src/context_version.rs` — wraps `Box<dyn ContextTransformer>`, holds `Arc<dyn ContextVersionStore>`, optional `Arc<dyn ContextSummarizer>`, internal `Mutex<VersioningState>` (next_version, turn_counter). Constructor `new()`, builder `with_summarizer()`, accessor `store()`
- [x] T040 [US4] Implement `ContextTransformer` for `VersioningTransformer` in `src/context_version.rs` — snapshot messages before inner transform, diff after to find dropped messages, save as `ContextVersion` with monotonic version number and Unix timestamp. Optionally summarize via `ContextSummarizer`
- [x] T041 [US4] Add re-exports to `src/lib.rs`: `pub use context_version::{ContextVersion, ContextVersionMeta, ContextVersionStore, InMemoryVersionStore, ContextSummarizer, VersioningTransformer};`

**Checkpoint**: `cargo test -p swink-agent context_version` passes. Versioning captures dropped messages, versions retrievable, summarizer integration works.

---

## Phase 7: User Story 5 — Explicit Context Caching with TTL (Priority: P1)

**Goal**: Provider-agnostic context caching abstraction with TTL, minimum token threshold, and cache interval lifecycle.

**Independent Test**: Configure `CacheConfig` with TTL=600s, min_tokens=4096, cache_intervals=3. Simulate 5 turns and verify Write/Read hint emission pattern.

### Implementation for User Story 5

- [x] T047 [US5] Create `src/context_cache.rs` with `#![forbid(unsafe_code)]`, module-level doc comment. Register in `src/lib.rs` as `mod context_cache;`
- [x] T051 [US5] Add unit tests for `CacheConfig`, `CacheHint`, and `CacheState` in `src/context_cache.rs`: (a) first turn emits Write, (b) turns 2..N emit Read, (c) turn N+1 emits Write (refresh), (d) `reset()` forces Write on next turn (adapter-reported cache miss), (e) `cached_prefix_len` tracks correctly, (f) `min_tokens` below threshold suppresses hints
- [x] T048 [US5] Implement `CacheConfig` struct in `src/context_cache.rs` — fields: `ttl: Duration`, `min_tokens: usize`, `cache_intervals: usize`. Derive `Debug, Clone`.
- [x] T049 [US5] Implement `CacheHint` enum in `src/context_cache.rs` — variants: `Write { ttl: Duration }`, `Read`. Derive `Debug, Clone, PartialEq`.
- [x] T050 [US5] Implement `CacheState` struct in `src/context_cache.rs` — fields: `turns_since_write: usize`, `cached_prefix_len: usize`. Methods: `new()`, `advance_turn(&mut self, config: &CacheConfig) -> CacheHint` (returns Write on first turn and every `cache_intervals` turns, Read otherwise), `reset(&mut self)` (force Write on next turn, used when adapter reports provider cache miss).
- [x] T074 [US5] Add unit tests for `cache_hint` field on `AgentMessage` in `tests/types.rs` or `src/types.rs`: (a) serde round-trip with `cache_hint: Some(Write)` preserves value, (b) serde round-trip with `cache_hint: None` omits field from JSON, (c) deserializing JSON without `cache_hint` field produces `None` (backward compat with old checkpoints).
- [x] T067 [US5] Add `cache_hint: Option<CacheHint>` field to `AgentMessage` in `src/types.rs` with `#[serde(default, skip_serializing_if = "Option::is_none")]`. Ensure backward-compatible deserialization.
- [x] T052 [US5] Add `cache_config: Option<CacheConfig>` field to `AgentOptions` in `src/agent_options.rs`. Add builder method `with_cache_config(config: CacheConfig)`.
- [x] T075 [US5] Add unit tests for sliding window cache prefix protection in `src/context_transformer.rs`: (a) `cached_prefix_len=3` with 10 messages — first 3 never compacted even if anchor=1, (b) `cached_prefix_len=0` — behaves identically to current (no regression), (c) `cached_prefix_len` larger than message count — all messages preserved.
- [x] T070 [US5] Modify `SlidingWindowTransformer` in `src/context_transformer.rs` to accept optional `cached_prefix_len: usize` and protect those messages from compaction (FR-016). When caching is active, anchor count is `max(anchor, cached_prefix_len)`.
- [x] T076 [US5] Add integration test for turn pipeline cache hint annotation and event emission in `tests/ac_context.rs` or new `tests/context_cache.rs`: (a) configure `CacheConfig` with `cache_intervals=2`, run 3 turns — verify turn 1 messages carry `CacheHint::Write`, turn 2 carry `CacheHint::Read`, turn 3 carry `CacheHint::Write` (refresh), (b) verify `AgentEvent::CacheAction` emitted each turn with correct hint and prefix_tokens, (c) no `CacheConfig` → no `CacheHint` on messages and no `CacheAction` events.
- [x] T068 [US5] Integrate cache hint annotation into the turn pipeline in `src/loop_/turn.rs` — after context transform, if `CacheConfig` is present: call `CacheState::advance_turn()`, annotate cacheable prefix messages with the returned `CacheHint`, set `cached_prefix_len` on `CacheState`.
- [x] T069 [US5] Add `CacheAction { hint: CacheHint, prefix_tokens: usize }` variant to `AgentEvent` in `src/types.rs`. Emit in `src/loop_/turn.rs` after cache hint annotation (FR-018).
- [x] T053 [US5] Add re-exports to `src/lib.rs`: `pub use context_cache::{CacheConfig, CacheHint, CacheState};`

**Checkpoint**: `cargo test -p swink-agent context_cache` passes. Cache lifecycle logic works standalone, messages annotated, sliding window respects cached prefix.

---

## Phase 8: User Story 6 — Static vs Dynamic Instruction Split (Priority: P1)

**Goal**: Separate static (cached) and dynamic (per-turn) system prompts on `AgentOptions`.

**Independent Test**: Create an agent with both static and dynamic prompts. Verify static portion is stable across turns, dynamic portion updates each turn.

### Implementation for User Story 6

- [x] T057 [US6] Add unit tests for prompt split in `src/agent_options.rs`: (a) only `system_prompt` set → used as-is, (b) `static_system_prompt` set → takes precedence as system prompt, (c) dynamic closure called fresh each invocation returns separate string, (d) `effective_system_prompt()` returns only static portion (not concatenated with dynamic).
- [x] T054 [US6] Add `static_system_prompt: Option<String>` and `dynamic_system_prompt: Option<Box<dyn Fn() -> String + Send + Sync>>` fields to `AgentOptions` in `src/agent_options.rs`.
- [x] T055 [US6] Add builder methods `.with_static_system_prompt(prompt: String)` and `.with_dynamic_system_prompt(f: impl Fn() -> String + Send + Sync + 'static)` to `AgentOptions`.
- [x] T056 [US6] Add `effective_system_prompt(&self) -> String` method to `AgentOptions` — returns `static_system_prompt` if set, otherwise falls back to `system_prompt`. Does NOT include dynamic content (dynamic prompt is injected as a separate user-role message by the turn pipeline).
- [x] T058 [US6] Update turn pipeline in `src/loop_/turn.rs` to: (a) use `effective_system_prompt()` for the system prompt message (cacheable), (b) if `dynamic_system_prompt` is set, inject its output as a separate user-role message immediately after the system prompt (non-cacheable).

**Checkpoint**: `cargo test -p swink-agent agent_options` passes. Static/dynamic split works with and without caching.

---

## Phase 9: User Story 7 — Context Overflow Predicate (Priority: P2)

**Goal**: `is_context_overflow()` function that estimates whether context exceeds the model's window before sending.

**Independent Test**: Create messages with known token estimate, a ModelSpec with `max_context_window = Some(1000)`, verify returns true/false correctly.

### Implementation for User Story 7

- [x] T059 [US7] Add unit tests for `is_context_overflow()` in `src/context.rs`: (a) messages within budget → false, (b) messages exceeding budget → true, (c) `max_context_window = None` → false, (d) custom TokenCounter used, (e) empty messages → false.
- [x] T060 [US7] Implement `is_context_overflow()` in `src/context.rs` — signature: `pub fn is_context_overflow(messages: &[AgentMessage], model: &ModelSpec, counter: Option<&dyn TokenCounter>) -> bool`. Sum token estimates, compare against `model.capabilities.as_ref().and_then(|c| c.max_context_window)`.
- [x] T061 [US7] Add re-export to `src/lib.rs`: `pub use context::is_context_overflow;`

**Checkpoint**: `cargo test -p swink-agent context` passes with new overflow predicate tests.

---

## Phase 9b: Cache-Miss Retry Integration (FR-019)

**Goal**: When an adapter returns a cache-miss error, the framework resets `CacheState` and retries with `CacheHint::Write`.

- [x] T071 [US5] Add `CacheMiss` variant to `AgentError` in `src/error.rs`. Mark as retryable in `is_retryable()` (alongside `ModelThrottled` and `NetworkError`).
- [x] T072 [US5] Add unit test for cache-miss retry in `src/loop_/turn.rs`: simulate a `CacheMiss` error, verify `CacheState::reset()` is called before retry and next attempt emits `CacheHint::Write`.
- [x] T073 [US5] Implement cache-miss detection in retry path in `src/loop_/turn.rs` — when `AgentError::CacheMiss` is caught, call `CacheState::reset()` before the retry turn so the next attempt re-sends the prefix with `CacheHint::Write`.

**Checkpoint**: Cache-miss → retry → Write cycle works end-to-end.

---

## Phase 10: Polish & Cross-Cutting Concerns

**Purpose**: Integration, API surface completeness, and validation across all stories

- [x] T042 Verify all public types are re-exported from `src/lib.rs` per contracts/public-api.md — cross-reference every type/function listed in the contract
- [x] T043 Run `cargo clippy --workspace -- -D warnings` and fix any warnings in context management modules
- [x] T044 Run `cargo test --workspace` to verify no regressions across the full workspace
- [x] T045 Run `cargo test -p swink-agent --no-default-features` to verify context management works with builtin-tools disabled
- [x] T046 Validate quickstart.md code examples compile by inspection — check `sliding_window`, `SlidingWindowTransformer`, `VersioningTransformer`, `convert_messages` usage patterns match the public API
- [x] T062 Verify new re-exports in `src/lib.rs`: `CacheConfig`, `CacheHint`, `CacheState`, `is_context_overflow`
- [x] T063 Run `cargo clippy --workspace -- -D warnings` and fix any warnings in new context cache and overflow modules
- [x] T064 Run `cargo test --workspace` to verify no regressions from new code
- [x] T065 Run `cargo test -p swink-agent --no-default-features` to verify caching and overflow work with builtin-tools disabled
- [x] T066 Validate new quickstart.md code examples compile by inspection — check `CacheConfig`, `is_context_overflow`, static/dynamic prompt patterns

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately ✅
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories ✅
- **User Story 1 (Phase 3)**: Depends on Foundational (Phase 2) ✅
- **User Story 2 (Phase 4)**: Depends on Phase 2 and Phase 3 ✅
- **User Story 3 (Phase 5)**: Depends on Phase 2 only ✅
- **User Story 4 (Phase 6)**: Depends on Phase 2 and Phase 4 ✅
- **User Story 5 (Phase 7)**: Depends on Phase 2 (TokenCounter for min_tokens check) — independent of US1-US4
- **User Story 6 (Phase 8)**: Depends on Phase 7 (CacheConfig must exist before prompt split references it) — can start in parallel with US7
- **User Story 7 (Phase 9)**: Depends on Phase 2 only (TokenCounter + ModelSpec) — independent of US5/US6
- **Cache-Miss Retry (Phase 9b)**: Depends on Phase 7 (CacheState, AgentError) — can run after US5 core is done
- **Polish (Phase 10)**: Depends on all user stories and Phase 9b being complete

### User Story Dependencies

- **US1 (Sliding Window)**: After Foundational — no story dependencies ✅
- **US2 (Transformers)**: After US1 ✅
- **US3 (Message Conversion)**: After Foundational ✅
- **US4 (Versioning)**: After US2 ✅
- **US5 (Context Caching)**: After Foundational — new module, no deps on US1-US4. Includes turn pipeline integration (T068), event emission (T069), sliding window protection (T070), and AgentMessage field (T067)
- **US6 (Static/Dynamic Split)**: After US5 — references CacheConfig for cache-aware prompt building
- **US7 (Overflow Predicate)**: After Foundational — independent of US5/US6, same file as token estimation
- **Cache-Miss Retry (Phase 9b)**: After US5 core — adds error variant and retry path integration

### Within Each User Story

- Types/structs before algorithms
- Core implementation before builder methods
- Tests before implementation (Constitution II: TDD — red-green-refactor)
- Re-exports after implementation

### Parallel Opportunities

- T003, T004, T005 can run in parallel (independent module files) ✅
- US1 and US3 can run in parallel after Foundational ✅
- US5 and US7 can run in parallel (different files, no dependencies)
- US6 depends on US5 but can overlap — T057 (tests) can start while US5 pipeline tasks run
- Phase 9b can run in parallel with US6 (different files: error.rs vs agent_options.rs)
- Within US5: T048, T049 can run in parallel (independent types); T067 can run in parallel with T048-T050 (different file: types.rs)
- Within US7: T059 and T060 are sequential (same function, TDD)

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

### MVP First (User Story 1 + User Story 3) ✅ COMPLETE

1. Complete Phase 1: Setup (module scaffolding) ✅
2. Complete Phase 2: Foundational (token estimation, result types) ✅
3. Complete Phase 3: User Story 1 (sliding window) ✅
4. Complete Phase 5: User Story 3 (message conversion) ✅
5. Core context management is functional ✅

### Incremental Delivery (Updated 2026-03-31)

1. Setup + Foundational → Foundation ready ✅
2. Add US1 (Sliding Window) → Core pruning works ✅
3. Add US3 (Message Conversion) → Provider pipeline works ✅
4. Add US2 (Transformers) → Pluggable transform hooks work ✅
5. Add US4 (Versioning) → Debug/observability layer works ✅
6. **Add US5 (Context Caching) → Cache lifecycle, hint emission, event, sliding window protection, AgentMessage field**
7. **Add US6 (Static/Dynamic Split) → Prompt separation for caching (dynamic as user-role message)**
8. **Add US7 (Overflow Predicate) → Pre-flight overflow detection** (can run in parallel with US5/US6)
9. **Add Phase 9b (Cache-Miss Retry) → Error variant + retry path integration**
10. Polish → Full workspace validation

### Parallel Team Strategy (New Stories)

- **Developer A**: US5 (Context Caching + pipeline integration) then US6 (Static/Dynamic Split) — sequential dependency
- **Developer B**: US7 (Overflow Predicate) then Phase 9b (Cache-Miss Retry, after US5 merges) — independent until 9b
- Both merge → Phase 10 Polish
