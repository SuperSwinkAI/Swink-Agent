# Tasks: Memory Crate

**Input**: Design documents from `/specs/021-memory-crate/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md, quickstart.md

**Tests**: Tests are included per user story. Integration tests use real filesystem I/O via `tempfile` (no mocks).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

**Important notes**:
- The existing scaffold code diverges from the spec in several ways (see Phase 2). All Phase 2 tasks must complete before any user story work begins.
- The core agent uses `AgentMessage` (not `LlmMessage`) in its `ContextTransformer` trait. The `SummarizingCompactor::compaction_fn()` closure signature must match `Fn(&mut Vec<AgentMessage>, bool)` — **not** the `Fn(Vec<LlmMessage>) -> Vec<LlmMessage>` stated in the contract. The contract should be updated to reflect this.
- The `CompactionResult` struct is defined in the contract but no method on `SummarizingCompactor` returns it. It may serve as a diagnostic return type for a future `compact()` method. Include it but mark it as non-essential.
- Spec acceptance scenarios US2-2 ("summary captures key topics") and US2-4 ("coherent follow-up") are integration-level concerns requiring a running agent loop. They are out of scope for this crate and deferred to 030-integration-tests.
- Spec edge case "concurrent writes — last-writer-wins" is a documented assumption, not an enforced guarantee. No test is needed; document the assumption.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

## Phase 1: Setup (Dependencies & Configuration)

**Purpose**: Ensure workspace dependencies and Cargo.toml are ready

- [ ] T001 Add `chrono` workspace dependency to root `Cargo.toml` (`[workspace.dependencies]` section) with `serde` feature enabled
- [ ] T002 Add `chrono` dependency to `memory/Cargo.toml` under `[dependencies]`
- [ ] T003 Verify `tracing` is listed in `memory/Cargo.toml` under `[dependencies]` (needed for corruption warnings)

---

## Phase 2: Foundation (Core Types Alignment)

**Purpose**: Update core types to match the data model and contracts. Fix all existing code that references the old types.

**CRITICAL**: No user story work can begin until this phase is complete. The existing scaffold code diverges from the spec — these tasks align it.

- [ ] T004 Update `SessionMeta` in `memory/src/meta.rs` to match the contract: fields `id: String`, `title: String`, `created_at: DateTime<Utc>`, `updated_at: DateTime<Utc>`. Remove `model`, `system_prompt`, `message_count`, `custom` fields. Derive `Serialize, Deserialize, PartialEq`. Update all construction sites: `store.rs` tests (lines 90-106), `store_async.rs` tests (lines 149-165), `jsonl.rs` save/load (lines 60-79), and `tests/session_roundtrip.rs`.
- [ ] T005 Update `memory/src/meta.rs` tests: serialization roundtrip with `DateTime<Utc>` timestamps. Remove old backward compat tests for `custom` field. Add new test verifying `PartialEq` works for test assertions.
- [ ] T006 [P] Add `validate_session_id()` function in `memory/src/jsonl.rs`: reject IDs containing `/`, `\`, `..`, null bytes with `io::ErrorKind::InvalidInput` error. Internal function, enforced at trait boundary on save/load/append/delete.
- [ ] T007 [P] Update `memory/src/time.rs`: add `pub fn now_utc() -> DateTime<Utc>` and `pub fn format_session_id() -> String` (YYYYMMDD_HHMMSS format using chrono). Remove `days_to_ymd` if fully replaced by chrono. Update tests.
- [ ] T008 [P] Add `CompactionResult` struct in `memory/src/compaction.rs`: `messages: Vec<AgentMessage>`, `removed_count: usize`, `summary: Option<String>`. Derive `Debug, Clone`. Note: no method currently returns this — it exists as a diagnostic type for future use.
- [ ] T009 Update `SessionStore` trait in `memory/src/store.rs`: change to `save(&self, id: &str, meta: &SessionMeta, messages: &[LlmMessage]) -> io::Result<()>`, `append(&self, id: &str, messages: &[LlmMessage]) -> io::Result<()>`, `load(&self, id: &str) -> io::Result<(SessionMeta, Vec<LlmMessage>)>`, `list`, `delete`. Remove `new_session_id()` from trait. Use `LlmMessage` instead of `AgentMessage`. Remove `SessionFilter` and `list_filtered` (not in contract). Delete associated `SessionFilter` tests.
- [ ] T010 Update `memory/src/lib.rs` re-exports: export `SessionMeta`, `SessionStore`, `AsyncSessionStore`, `JsonlSessionStore`, `SummarizingCompactor`, `CompactionResult`, and time utilities (`now_utc`, `format_session_id`). Rename `SessionStoreAsync` → `AsyncSessionStore`. Keep `BlockingSessionStore`.
- [ ] T011 Update existing test file `memory/tests/session_roundtrip.rs`: align with new API signatures (new `SessionMeta` fields, `LlmMessage` instead of `AgentMessage`, new `save()` signature). Or delete and replace with `memory/tests/round_trip.rs` in Phase 3.
- [ ] T012 [P] Update `memory/CLAUDE.md` with any new conventions or lessons learned during foundation work

**Checkpoint**: Core types aligned with spec — ready for user story implementation

---

## Phase 3: User Story 1 — Save and Load Conversation Sessions (Priority: P1)

**Goal**: Developer saves a full conversation and loads it back with identical content and metadata.

**Independent Test**: Create a session with several messages, save it, load it back, verify message log and metadata are identical.

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation. All tests must use `LlmMessage` (not `AgentMessage`) per the contract.**

- [ ] T013 [P] [US1] Integration test `save_and_load_roundtrip` in `memory/tests/round_trip.rs`: create `JsonlSessionStore` with tempdir, build `SessionMeta` and `Vec<LlmMessage>`, save, load, assert messages match in order and metadata (title, timestamps) is preserved
- [ ] T014 [P] [US1] Integration test `save_overwrites_existing_session` in `memory/tests/round_trip.rs`: save a session, save again with same ID and different messages, load and verify only new messages are present
- [ ] T015 [P] [US1] Integration test `load_nonexistent_session_returns_not_found` in `memory/tests/round_trip.rs`: attempt to load a session ID that was never saved, assert `io::ErrorKind::NotFound`
- [ ] T016 [P] [US1] Integration test `load_empty_file_returns_invalid_data` in `memory/tests/round_trip.rs`: create an empty `.jsonl` file in the store directory, attempt to load it, assert `io::ErrorKind::InvalidData`. (Also covers spec edge case: empty file)
- [ ] T017 [P] [US1] Integration test `save_filters_custom_messages` in `memory/tests/round_trip.rs`: save a message list that includes both `LlmMessage` variants, load back, verify content preserved (note: `LlmMessage` has no `CustomMessage` variant — this test verifies the type boundary is correct)
- [ ] T018 [P] [US1] Unit test `invalid_session_id_rejected` in `memory/src/jsonl.rs` tests: attempt save/load with IDs containing `/`, `\`, `..`, `\0`, assert `io::ErrorKind::InvalidInput`

### Implementation for User Story 1

- [ ] T019 [US1] Update `JsonlSessionStore::save()` in `memory/src/jsonl.rs`: accept `meta: &SessionMeta` and `messages: &[LlmMessage]`, validate session ID via `validate_session_id()`, write meta as line 1, write each `LlmMessage` as subsequent lines
- [ ] T020 [US1] Update `JsonlSessionStore::load()` in `memory/src/jsonl.rs`: validate session ID, read line 1 as `SessionMeta`, read remaining lines as `LlmMessage` values, return `(SessionMeta, Vec<LlmMessage>)`. Return `NotFound` for missing file, `InvalidData` for empty file.
- [ ] T021 [US1] Move `new_session_id()` off the `SessionStore` trait: either keep as a method on `JsonlSessionStore` that delegates to `format_session_id()`, or remove entirely and have callers use `time::format_session_id()` directly. Keep `JsonlSessionStore::default_dir()` as a convenience method (not in contract but useful).
- [ ] T022 [US1] Document single-writer assumption in `JsonlSessionStore` doc comment: "Concurrent writes to the same session may corrupt the file. Callers are expected to enforce single-writer access."

**Checkpoint**: US1 complete — save/load roundtrip works with correct metadata

---

## Phase 4: User Story 2 — Compact Long Conversations via Summarization (Priority: P1)

**Goal**: Long conversations are compacted by replacing older messages with a summary, retaining recent messages verbatim.

**Independent Test**: Provide a long message log, run the compactor, verify output contains a summary prefix followed by recent messages within budget.

**Note on closure signature**: The core agent's `ContextTransformer` trait uses `fn transform(&self, messages: &mut Vec<AgentMessage>, overflow: bool)` with a blanket impl for `Fn(&mut Vec<AgentMessage>, bool)`. The existing `SummarizingCompactor::compaction_fn()` already returns this correct type. The contract's `Fn(Vec<LlmMessage>) -> Vec<LlmMessage>` is incorrect and should be updated.

### Tests for User Story 2

- [ ] T023 [P] [US2] Unit test `compaction_within_budget_returns_unchanged` in `memory/src/compaction.rs` tests: create messages within budget, run `compaction_fn`, verify messages returned unchanged (no summary injection even if one is stored). Already exists as `no_compaction_needed_no_summary_injected` — verify coverage.
- [ ] T024 [P] [US2] Unit test `compaction_exceeding_budget_replaces_older_with_summary` in `memory/src/compaction.rs` tests: create messages exceeding budget, set summary, run `compaction_fn`, verify older messages replaced by summary + recent messages retained. Already exists as `with_summary_injects_after_anchor` — verify coverage.
- [ ] T025 [P] [US2] Unit test `summary_injected_as_assistant_message` in `memory/src/compaction.rs` tests: verify the injected summary is an `AssistantMessage` with text containing the summary content. The summary text is passed through verbatim from `set_summary()` — the summary content quality depends on the caller-provided summarization function (out of scope).
- [ ] T026 [P] [US2] Unit test `compaction_with_single_message_returns_unchanged` in `memory/src/compaction.rs` tests: single-message conversation within budget, run compaction, verify returned unchanged
- [ ] T027 [P] [US2] Unit test `summary_consumed_after_injection` in `memory/src/compaction.rs` tests: set summary, trigger compaction that drops messages, verify `has_summary()` returns false afterward (summary consumed on use, reset to `None`)

### Implementation for User Story 2

- [ ] T028 [US2] Update `SummarizingCompactor::compaction_fn()` in `memory/src/compaction.rs` to consume the summary after injection: after the summary is injected, set the internal `Option<String>` to `None` so the same summary is not re-injected on subsequent calls. This requires changing from a shared `&self` read to a write lock inside the closure.
- [ ] T029 [US2] Verify `set_summary` is thread-safe via `Arc<Mutex<>>` with `PoisonError::into_inner()` — already implemented, verify correctness
- [ ] T030 [US2] Verify existing `compaction_fn()` closure is compatible with `Agent::with_transform_context()` — the closure takes `(&mut Vec<AgentMessage>, bool)` which matches the `ContextTransformer` blanket impl. No changes needed if correct.

**Checkpoint**: US2 complete — long conversations compacted with summary injection

---

## Phase 5: User Story 3 — Perform Store Operations Asynchronously (Priority: P2)

**Goal**: Session store supports async save, load, list, and delete for non-blocking I/O.

**Independent Test**: Perform concurrent async save and load operations, verify data integrity.

### Tests for User Story 3

- [ ] T031 [P] [US3] Async integration test `async_save_and_load_roundtrip` in `memory/tests/async_store.rs`: use `JsonlSessionStore` with tempdir via `BlockingSessionStore` adapter, async save and load, verify identical content
- [ ] T032 [P] [US3] Async integration test `concurrent_async_operations_on_different_sessions` in `memory/tests/async_store.rs`: spawn multiple async save/load tasks on different session IDs concurrently, verify all complete successfully
- [ ] T033 [P] [US3] Async integration test `async_list_and_delete` in `memory/tests/async_store.rs`: async save multiple sessions, async list to verify all present, async delete one, async list to verify deleted

### Implementation for User Story 3

- [ ] T034 [US3] Rename `SessionStoreAsync` → `AsyncSessionStore` in `memory/src/store_async.rs`. Update trait methods to match contract: `save(&self, id, meta, messages)`, `append(&self, id, messages)`, `load`, `list`, `delete`. Remove `new_session_id()` from trait. Use `LlmMessage` instead of `AgentMessage`. Update existing tests in `memory/src/store_async.rs` to match new signatures.
- [ ] T035 [US3] Update `BlockingSessionStore` adapter in `memory/src/store_async.rs` to match new `SessionStore` and `AsyncSessionStore` signatures. Remove `new_session_id()` delegation. Remove `CustomMessage` filtering logic (no longer needed since trait uses `LlmMessage`).
- [ ] T036 [US3] Update `memory/src/lib.rs` re-exports to use new trait name `AsyncSessionStore`

**Checkpoint**: US3 complete — async operations work without blocking

---

## Phase 6: User Story 4 — List and Delete Sessions (Priority: P2)

**Goal**: Developer can browse and clean up saved sessions.

**Independent Test**: Create sessions, list them, verify metadata, delete one, confirm removal.

### Tests for User Story 4

- [ ] T037 [P] [US4] Integration test `list_returns_all_sessions_with_metadata` in `memory/tests/round_trip.rs`: save 3 sessions with distinct titles, list, verify all 3 returned with correct metadata
- [ ] T038 [P] [US4] Integration test `list_sorted_by_most_recent` in `memory/tests/round_trip.rs`: save sessions with different timestamps, list, verify sorted by `updated_at` descending
- [ ] T039 [P] [US4] Integration test `delete_removes_session` in `memory/tests/round_trip.rs`: save a session, delete it, verify load returns `NotFound` and list does not include it
- [ ] T040 [P] [US4] Integration test `list_empty_store_returns_empty` in `memory/tests/round_trip.rs`: create store with empty tempdir, list, verify empty vec returned

### Implementation for User Story 4

- [ ] T041 [US4] Update `JsonlSessionStore::list()` in `memory/src/jsonl.rs`: read first line of each `.jsonl` file as `SessionMeta`, sort by `updated_at` descending, skip files that fail to parse (log `tracing::warn!`)
- [ ] T042 [US4] Update `JsonlSessionStore::delete()` in `memory/src/jsonl.rs`: validate session ID before deletion, return appropriate error if file doesn't exist

**Checkpoint**: US4 complete — session listing and deletion work correctly

---

## Phase 7: User Story 5 — JSONL Format with Corruption Recovery (Priority: P3)

**Goal**: Sessions stored in human-readable JSONL format with partial corruption recovery and append-only writes.

**Independent Test**: Save session, inspect raw file, corrupt one line, verify remaining messages recoverable.

### Tests for User Story 5

- [ ] T043 [P] [US5] Integration test `jsonl_file_is_human_readable` in `memory/tests/corruption.rs`: save a session, read the raw `.jsonl` file, verify each line is independently parseable JSON
- [ ] T044 [P] [US5] Integration test `corrupted_line_recovers_remaining_messages` in `memory/tests/corruption.rs`: save a session with 5 messages, manually corrupt one line in the middle of the file, load, verify 4 messages recovered and a warning was logged
- [ ] T045 [P] [US5] Integration test `all_message_lines_corrupted_returns_empty_messages` in `memory/tests/corruption.rs`: create a `.jsonl` file where line 1 (meta) is valid but all message lines are corrupted, load, verify meta is returned with 0 messages (not an error)
- [ ] T046 [P] [US5] Integration test `append_does_not_rewrite_file` in `memory/tests/corruption.rs`: save session with 3 messages, note file size, append 2 more messages, verify file size grew (not rewritten), load and verify all 5 messages present

### Implementation for User Story 5

- [ ] T047 [US5] Update `JsonlSessionStore::load()` corruption handling in `memory/src/jsonl.rs`: when a message line fails to parse, log `tracing::warn!` with the line number and error, skip the line, continue loading remaining lines. If metadata line (line 1) fails, return `io::ErrorKind::InvalidData`.
- [ ] T048 [US5] Implement `JsonlSessionStore::append()` in `memory/src/jsonl.rs`: validate session ID, open existing session file in append mode, write new `LlmMessage` values as additional lines, update `SessionMeta` by rewriting line 1 with new `updated_at` timestamp. Document single-writer assumption.
- [ ] T049 [US5] Implement `AsyncSessionStore::append()` in `memory/src/store_async.rs` via `BlockingSessionStore` delegation

**Checkpoint**: US5 complete — JSONL format with corruption tolerance and append-only writes

---

## Phase 8: Polish & Integration

**Purpose**: Final cleanup, documentation, and build verification

- [ ] T050 [P] Add `memory/tests/common/mod.rs` with shared test helpers: functions to create sample `LlmMessage` values (user message, assistant message with content blocks), sample `SessionMeta` with `DateTime<Utc>`, and tempdir setup
- [ ] T051 [P] Review `memory/src/compaction.rs` imports: ensure they reference correct types from `swink_agent` (`AgentMessage`, `LlmMessage`, `AssistantMessage`, `ContentBlock`, etc.)
- [ ] T052 Verify `#![forbid(unsafe_code)]` is present at `memory/src/lib.rs` crate root
- [ ] T053 Run `cargo build -p swink-agent-memory` — fix any compilation errors
- [ ] T054 Run `cargo test -p swink-agent-memory` — fix any test failures
- [ ] T055 Run `cargo clippy -p swink-agent-memory -- -D warnings` — fix any warnings
- [ ] T056 Run `cargo test --workspace` — verify no regressions in other crates
- [ ] T057 Run `cargo clippy --workspace -- -D warnings` — verify no workspace-wide warnings

**Checkpoint**: All tests pass, clippy clean, workspace builds successfully
