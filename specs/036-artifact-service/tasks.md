# Tasks: Artifact Service

**Input**: Design documents from `/specs/036-artifact-service/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md, contracts/

**Tests**: Included — constitution mandates test-driven development (NON-NEGOTIABLE).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create the `swink-agent-artifacts` workspace crate and add feature gates to the core crate.

- [ ] T001 Create `artifacts/` directory and `artifacts/Cargo.toml` with workspace deps (`serde`, `serde_json`, `tokio`, `chrono`, `tracing`, `thiserror`, `futures`, `bytes`) and dependency on `swink-agent` with `artifact-store` feature
- [ ] T002 Add `"artifacts"` to workspace members in root `Cargo.toml`
- [ ] T003 Add `artifact-store` and `artifact-tools` features to core crate `Cargo.toml` (`artifact-tools` depends on `artifact-store`); add `chrono` dep (already workspace dep)
- [ ] T004 Create `artifacts/src/lib.rs` with `#![forbid(unsafe_code)]` and placeholder module declarations
- [ ] T005 Create `src/artifact.rs` with `#[cfg(feature = "artifact-store")]` module stub in `src/lib.rs`

**Checkpoint**: Workspace compiles with `cargo build --workspace`. Feature gates are wired but empty.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core types, trait, error enum, and name validation that ALL user stories depend on.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [ ] T006 Define `ArtifactError` enum (`InvalidName`, `Storage`, `NotConfigured`) with `thiserror` derives in `src/artifact.rs`
- [ ] T007 [P] Define `ArtifactData` struct (`content: Vec<u8>`, `content_type: String`, `metadata: HashMap<String, String>`) with serde derives in `src/artifact.rs`
- [ ] T008 [P] Define `ArtifactVersion` struct (`name`, `version: u32`, `created_at: DateTime<Utc>`, `size: usize`, `content_type`) with serde derives in `src/artifact.rs`
- [ ] T009 [P] Define `ArtifactMeta` struct (`name`, `latest_version: u32`, `created_at`, `updated_at`, `content_type`) with serde derives in `src/artifact.rs`
- [ ] T010 Define `ArtifactStore` async trait (`save`, `load`, `load_version`, `list`, `delete`) with `Send + Sync` bounds in `src/artifact.rs` per contracts/public-api.md signatures
- [ ] T011 Implement `validate_artifact_name` function in `artifacts/src/validate.rs` — allowed chars `[a-zA-Z0-9\-_./]`, no empty, no leading/trailing `/`, no `//`
- [ ] T012 [P] Write unit tests for `validate_artifact_name` in `artifacts/src/validate.rs` — valid names, invalid chars, empty, leading/trailing slash, consecutive slashes, path traversal (`../`)
- [ ] T013 Add `AgentEvent::ArtifactSaved { session_id, name, version }` variant to `src/loop_/event.rs` behind `#[cfg(feature = "artifact-store")]`
- [ ] T014 Add re-exports of `ArtifactStore`, `ArtifactData`, `ArtifactVersion`, `ArtifactMeta`, `ArtifactError`, `validate_artifact_name` in `src/lib.rs` behind `#[cfg(feature = "artifact-store")]`
- [ ] T015 Verify `cargo build --workspace` and `cargo test --workspace` pass with both features enabled and disabled

**Checkpoint**: Foundation ready — core trait and types compile, validation tested, event variant wired.

---

## Phase 3: User Story 1 — Agent Tool Saves a Generated File as an Artifact (Priority: P1) 🎯 MVP

**Goal**: Tools can save versioned artifacts attached to a session. Saving the same name creates a new version; all versions remain accessible.

**Independent Test**: Create `InMemoryArtifactStore`, save two versions of "report.md", verify both are independently retrievable with correct content and version numbers.

### Tests for User Story 1

- [ ] T016 [P] [US1] Write test `save_creates_version_one` in `artifacts/tests/memory_store.rs` — save artifact, assert version 1, correct size/content_type
- [ ] T017 [P] [US1] Write test `save_same_name_increments_version` in `artifacts/tests/memory_store.rs` — save twice, assert versions 1 and 2
- [ ] T018 [P] [US1] Write test `load_returns_latest_version` in `artifacts/tests/memory_store.rs` — save 3 versions, `load` returns version 3
- [ ] T019 [P] [US1] Write test `load_version_returns_specific` in `artifacts/tests/memory_store.rs` — save 3 versions, `load_version(1)` returns version 1 content
- [ ] T020 [P] [US1] Write test `load_nonexistent_returns_none` in `artifacts/tests/memory_store.rs` — `load` unknown name returns `None`
- [ ] T021 [P] [US1] Write test `load_version_nonexistent_returns_none` in `artifacts/tests/memory_store.rs` — `load_version(99)` returns `None`
- [ ] T022 [P] [US1] Write test `save_validates_name` in `artifacts/tests/memory_store.rs` — invalid name returns `ArtifactError::InvalidName`
- [ ] T023 [P] [US1] Write test `save_empty_content_succeeds` in `artifacts/tests/memory_store.rs` — zero-byte artifact saves correctly

### Implementation for User Story 1

- [ ] T024 [US1] Implement `InMemoryArtifactStore` struct in `artifacts/src/memory_store.rs` — `Arc<tokio::sync::Mutex<HashMap<String, HashMap<String, Vec<(ArtifactVersion, ArtifactData)>>>>>`, `new()` constructor
- [ ] T025 [US1] Implement `ArtifactStore::save` for `InMemoryArtifactStore` in `artifacts/src/memory_store.rs` — validate name, increment version, store data, emit `tracing::debug!`
- [ ] T026 [US1] Implement `ArtifactStore::load` and `ArtifactStore::load_version` for `InMemoryArtifactStore` in `artifacts/src/memory_store.rs`
- [ ] T027 [US1] Re-export `InMemoryArtifactStore` from `artifacts/src/lib.rs`
- [ ] T028 [US1] Run all US1 tests — verify they pass

**Checkpoint**: `InMemoryArtifactStore` saves and loads versioned artifacts. All US1 tests pass.

---

## Phase 4: User Story 2 — Agent Lists and Loads Artifacts from a Session (Priority: P1)

**Goal**: Consumers can list all artifacts in a session with metadata (name, version count, content type, timestamps) without loading content. Loading returns full content + metadata.

**Independent Test**: Save three artifacts with different content types, call `list`, verify all three appear with correct metadata. Load each and verify content integrity.

### Tests for User Story 2

- [ ] T029 [P] [US2] Write test `list_returns_all_artifacts` in `artifacts/tests/memory_store.rs` — save 3 artifacts, list returns 3 entries with correct names/versions/types
- [ ] T030 [P] [US2] Write test `list_empty_session_returns_empty` in `artifacts/tests/memory_store.rs` — list on unknown session returns empty vec
- [ ] T031 [P] [US2] Write test `list_reflects_latest_version` in `artifacts/tests/memory_store.rs` — save 2 versions of same artifact, list shows `latest_version: 2`
- [ ] T032 [P] [US2] Write test `load_includes_custom_metadata` in `artifacts/tests/memory_store.rs` — save with metadata map, load returns same metadata

### Implementation for User Story 2

- [ ] T033 [US2] Implement `ArtifactStore::list` for `InMemoryArtifactStore` in `artifacts/src/memory_store.rs` — iterate artifacts, build `ArtifactMeta` with timestamps from version records
- [ ] T034 [US2] Run all US2 tests — verify they pass

**Checkpoint**: List returns correct metadata for all artifacts. US1 + US2 tests pass.

---

## Phase 5: User Story 3 — Artifact Store Persists Across Session Boundaries (Priority: P1)

**Goal**: Artifacts saved via `FileArtifactStore` persist on disk and survive session restore. A new agent with the same session ID and store can access all previously saved artifacts.

**Independent Test**: Save artifacts to `FileArtifactStore` with a temp dir, drop the store, create a new one at the same path, verify all artifacts loadable with byte-for-byte content integrity.

### Tests for User Story 3

- [ ] T035 [P] [US3] Write test `fs_save_and_load_round_trip` in `artifacts/tests/fs_store.rs` — save artifact, load back, assert content matches byte-for-byte (use `tempfile::TempDir`)
- [ ] T036 [P] [US3] Write test `fs_persistence_across_instances` in `artifacts/tests/fs_store.rs` — save with store A, drop A, create store B at same path, load succeeds
- [ ] T037 [P] [US3] Write test `fs_versioning_persists` in `artifacts/tests/fs_store.rs` — save 3 versions, recreate store, all 3 versions accessible
- [ ] T038 [P] [US3] Write test `fs_large_artifact_integrity` in `artifacts/tests/fs_store.rs` — save 1MB random bytes, load back, assert identical
- [ ] T039 [P] [US3] Write test `fs_concurrent_saves_no_corruption` in `artifacts/tests/fs_store.rs` — spawn 10 concurrent saves to same artifact name, verify all 10 versions exist with no data corruption
- [ ] T040 [P] [US3] Write test `fs_empty_session_returns_empty` in `artifacts/tests/fs_store.rs` — list/load on fresh store returns empty/None

### Implementation for User Story 3

- [ ] T041 [US3] Implement `FileArtifactStore` struct in `artifacts/src/fs_store.rs` — `root: PathBuf`, `locks: Arc<tokio::sync::Mutex<HashMap<(String, String), Arc<tokio::sync::Mutex<()>>>>>`, `new()` constructor
- [ ] T042 [US3] Implement `ArtifactStore::save` for `FileArtifactStore` in `artifacts/src/fs_store.rs` — validate name, acquire per-artifact lock, read/create meta.json, write content to `v{N}.bin` via temp-file + atomic rename, update meta.json, `tracing::info!`
- [ ] T043 [US3] Implement `ArtifactStore::load` and `ArtifactStore::load_version` for `FileArtifactStore` in `artifacts/src/fs_store.rs` — read meta.json for version lookup, read `v{N}.bin` content
- [ ] T044 [US3] Implement `ArtifactStore::list` for `FileArtifactStore` in `artifacts/src/fs_store.rs` — scan session directory, read each artifact's meta.json, build `ArtifactMeta` entries
- [ ] T045 [US3] Re-export `FileArtifactStore` from `artifacts/src/lib.rs`
- [ ] T046 [US3] Run all US3 tests — verify they pass

**Checkpoint**: `FileArtifactStore` persists artifacts to disk. Concurrent saves produce sequential versions. All P1 story tests pass.

---

## Phase 6: User Story 4 — LLM Agent Uses Built-in Tools to Manage Artifacts (Priority: P2)

**Goal**: Built-in `save_artifact`, `load_artifact`, and `list_artifacts` tools let the LLM autonomously manage artifacts during a conversation.

**Independent Test**: Configure agent with built-in artifact tools and `InMemoryArtifactStore`. Call save tool, then load tool, verify results. Verify tools absent when feature disabled.

### Tests for User Story 4

- [ ] T047 [P] [US4] Write test `save_artifact_tool_creates_version` in `src/tools/tests/artifact_tools.rs` — call tool with name/content, verify artifact saved in store
- [ ] T048 [P] [US4] Write test `load_artifact_tool_returns_text_content` in `src/tools/tests/artifact_tools.rs` — save text artifact, call load tool, verify text content in result
- [ ] T049 [P] [US4] Write test `load_artifact_tool_returns_binary_summary` in `src/tools/tests/artifact_tools.rs` — save binary artifact, call load tool, verify `"[binary: N bytes, type: ...]"` summary
- [ ] T050 [P] [US4] Write test `list_artifacts_tool_returns_formatted_list` in `src/tools/tests/artifact_tools.rs` — save 2 artifacts, call list tool, verify formatted output
- [ ] T051 [P] [US4] Write test `list_artifacts_tool_empty_session` in `src/tools/tests/artifact_tools.rs` — call list on empty session, verify "No artifacts" message
- [ ] T052 [P] [US4] Write test `artifact_tools_convenience_constructor` in `src/tools/tests/artifact_tools.rs` — call `artifact_tools()`, verify returns 3 tools with correct names

### Implementation for User Story 4

- [ ] T053 [US4] Implement `SaveArtifactTool` in `src/tools/save_artifact.rs` — captures `Arc<dyn ArtifactStore>`, accepts `name`/`content`/`content_type` params, implements `AgentTool` with `JsonSchema`-derived params, returns version confirmation
- [ ] T054 [US4] Implement `LoadArtifactTool` in `src/tools/load_artifact.rs` — captures `Arc<dyn ArtifactStore>`, accepts `name`/`version` params, returns text content or binary summary
- [ ] T055 [US4] Implement `ListArtifactsTool` in `src/tools/list_artifacts.rs` — captures `Arc<dyn ArtifactStore>`, no required params, returns formatted artifact list
- [ ] T056 [US4] Implement `artifact_tools(store) -> Vec<Box<dyn AgentTool>>` convenience constructor in `src/tools/mod.rs`
- [ ] T057 [US4] Add `#[cfg(feature = "artifact-tools")]` module declarations for `save_artifact`, `load_artifact`, `list_artifacts` in `src/tools/mod.rs`
- [ ] T058 [US4] Add re-exports of `SaveArtifactTool`, `LoadArtifactTool`, `ListArtifactsTool`, `artifact_tools` in `src/lib.rs` behind `#[cfg(feature = "artifact-tools")]`
- [ ] T059 [US4] Run all US4 tests — verify they pass
- [ ] T060 [US4] Verify `cargo build -p swink-agent --no-default-features` still compiles (tools not pulled in)

**Checkpoint**: LLM can save, load, and list artifacts via built-in tools. Feature gate verified.

---

## Phase 7: User Story 5 — Streaming Large Artifacts (Priority: P2)

**Goal**: `StreamingArtifactStore` extension trait enables memory-efficient I/O for large artifacts. `FileArtifactStore` implements it.

**Independent Test**: Save 10MB artifact via `save_stream`, load via `load_stream`, verify byte-for-byte integrity.

### Tests for User Story 5

- [ ] T061 [P] [US5] Write test `streaming_save_round_trip` in `artifacts/tests/streaming.rs` — stream 10MB in chunks, load via `load_stream`, verify content matches
- [ ] T062 [P] [US5] Write test `streaming_save_creates_version` in `artifacts/tests/streaming.rs` — save via stream, verify `ArtifactVersion` returned with correct size
- [ ] T063 [P] [US5] Write test `streaming_load_nonexistent_returns_none` in `artifacts/tests/streaming.rs` — `load_stream` on unknown artifact returns `None`
- [ ] T064 [P] [US5] Write test `non_streaming_api_still_works` in `artifacts/tests/streaming.rs` — save via base `Vec<u8>` API, load via streaming, verify compatible

### Implementation for User Story 5

- [ ] T065 [US5] Define `StreamingArtifactStore` extension trait in `src/artifact.rs` — `save_stream` and `load_stream` methods per contracts/public-api.md signatures, behind `artifact-store` feature
- [ ] T066 [US5] Implement `StreamingArtifactStore` for `FileArtifactStore` in `artifacts/src/streaming.rs` — buffered `tokio::fs` read/write, chunk size 64KB
- [ ] T067 [US5] Add re-export of `StreamingArtifactStore` in `src/lib.rs` behind `#[cfg(feature = "artifact-store")]`
- [ ] T068 [US5] Run all US5 tests — verify they pass

**Checkpoint**: Large artifacts stream through `FileArtifactStore` without full-content heap allocation.

---

## Phase 8: User Story 6 — Artifact Deletion (Priority: P3)

**Goal**: Consumers can delete all versions of a named artifact. Deletion is idempotent.

**Independent Test**: Save artifact with 3 versions, delete, verify list no longer includes it, load returns `None`.

### Tests for User Story 6

- [ ] T069 [P] [US6] Write test `delete_removes_all_versions` in `artifacts/tests/memory_store.rs` — save 3 versions, delete, load returns `None`, list excludes it
- [ ] T070 [P] [US6] Write test `delete_nonexistent_succeeds` in `artifacts/tests/memory_store.rs` — delete unknown name succeeds silently
- [ ] T071 [P] [US6] Write test `fs_delete_removes_files` in `artifacts/tests/fs_store.rs` — save artifact, delete, verify directory/files removed from disk
- [ ] T072 [P] [US6] Write test `fs_delete_nonexistent_succeeds` in `artifacts/tests/fs_store.rs` — delete on empty store succeeds

### Implementation for User Story 6

- [ ] T073 [US6] Implement `ArtifactStore::delete` for `InMemoryArtifactStore` in `artifacts/src/memory_store.rs` — remove entry from hashmap
- [ ] T074 [US6] Implement `ArtifactStore::delete` for `FileArtifactStore` in `artifacts/src/fs_store.rs` — remove artifact directory and all contents
- [ ] T075 [US6] Run all US6 tests — verify they pass

**Checkpoint**: Deletion works for both store implementations. All user story tests pass.

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Integration, configuration hookup, and workspace-level validation.

- [ ] T076 [P] Add optional `artifact_store: Option<Arc<dyn ArtifactStore>>` field to `AgentOptions` in `src/agent_options.rs` behind `#[cfg(feature = "artifact-store")]`
- [ ] T077 [P] Verify `cargo clippy --workspace -- -D warnings` passes with zero warnings
- [ ] T078 [P] Verify `cargo test --workspace` passes (all crates, all features)
- [ ] T079 [P] Verify `cargo build -p swink-agent --no-default-features` compiles (no artifact deps pulled in)
- [ ] T080 [P] Verify `cargo build -p swink-agent --features artifact-store` compiles without `artifact-tools`
- [ ] T081 Run quickstart.md code examples validation — verify examples compile and demonstrate correct usage patterns

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **User Stories (Phase 3–8)**: All depend on Foundational phase completion
  - US1 (Save/Load): Can start after Phase 2
  - US2 (List): Depends on US1 (needs `InMemoryArtifactStore` from US1)
  - US3 (Persistence): Depends on US1 (needs trait + types; builds `FileArtifactStore`)
  - US4 (Tools): Depends on US1 (needs `ArtifactStore` trait + `InMemoryArtifactStore` for testing)
  - US5 (Streaming): Depends on US3 (needs `FileArtifactStore`)
  - US6 (Deletion): Depends on US1 + US3 (adds `delete` to both stores)
- **Polish (Phase 9)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (P1)**: Phase 2 only — no other story dependencies
- **US2 (P1)**: US1 (uses `InMemoryArtifactStore` and `save` to set up test data)
- **US3 (P1)**: US1 (reuses trait types; `FileArtifactStore` implements same trait)
- **US4 (P2)**: US1 (tools reference `ArtifactStore` trait)
- **US5 (P2)**: US3 (implements streaming on `FileArtifactStore`)
- **US6 (P3)**: US1 + US3 (adds `delete` method to both store implementations)

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Types/models before trait implementations
- Trait implementations before integration
- Story complete before moving to next priority

### Parallel Opportunities

- T007, T008, T009 (type definitions) can run in parallel
- All US1 tests (T016–T023) can run in parallel
- All US2 tests (T029–T032) can run in parallel
- US3 tests (T035–T040) and US4 tests (T047–T052) can run in parallel after their dependencies
- US6 tests (T069–T072) can all run in parallel
- Phase 9 tasks (T076–T080) can all run in parallel

---

## Parallel Example: User Story 1

```bash
# Write all US1 tests in parallel (different test functions, same file):
T016: save_creates_version_one
T017: save_same_name_increments_version
T018: load_returns_latest_version
T019: load_version_returns_specific
T020: load_nonexistent_returns_none
T021: load_version_nonexistent_returns_none
T022: save_validates_name
T023: save_empty_content_succeeds

# Then implement sequentially:
T024: InMemoryArtifactStore struct
T025: save implementation
T026: load + load_version implementation
T027: Re-exports
T028: Run all tests
```

---

## Implementation Strategy

### MVP First (User Stories 1–3 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: US1 (save/load/version)
4. Complete Phase 4: US2 (list with metadata)
5. Complete Phase 5: US3 (filesystem persistence)
6. **STOP and VALIDATE**: All P1 stories functional, both store implementations working

### Incremental Delivery

1. Setup + Foundational → trait and types compile
2. US1 → `InMemoryArtifactStore` saves/loads versioned artifacts (MVP!)
3. US2 → List with metadata works
4. US3 → `FileArtifactStore` persists to disk
5. US4 → LLM can use artifact tools
6. US5 → Streaming for large artifacts
7. US6 → Cleanup/deletion
8. Polish → Integration, clippy, feature gate validation

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Constitution requires TDD: write failing tests before implementation
- `#[forbid(unsafe_code)]` on artifacts crate root
- All store implementations must be `Send + Sync`
- `InMemoryArtifactStore` is always available (not feature-gated in artifacts crate)
- `FileArtifactStore` uses temp-file + rename for atomic writes
- Commit after each task or logical group
