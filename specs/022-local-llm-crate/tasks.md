# Tasks: Local LLM Crate

**Input**: Design documents from `/specs/022-local-llm-crate/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Crate scaffolding, workspace integration, and dependency configuration

- [ ] T001 Verify `local-llm/Cargo.toml` declares correct workspace dependencies (`mistralrs`, `hf-hub`, `tokio`, `tokio-stream`, `futures`, `serde`, `serde_json`, `thiserror`, `tracing`, `uuid`) and add any missing entries to the root `Cargo.toml` `[workspace.dependencies]`
- [ ] T002 Verify `local-llm/src/lib.rs` has `#![forbid(unsafe_code)]` and re-exports all public types (`LocalModel`, `LocalStreamFn`, `EmbeddingModel`, `ModelPreset`, `ModelConfig`, `ModelState`, `ProgressCallbackFn`, `ProgressEvent`, `LocalModelError`)
- [ ] T003 [P] Create `local-llm/tests/common/mod.rs` with shared test helpers (mock progress callback collector, test `ModelConfig` factory)

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Error types, progress reporting, presets, and config — types that ALL user stories depend on

**CRITICAL**: No user story work can begin until this phase is complete

- [ ] T004 Implement `LocalModelError` enum with `Download`, `Loading`, `Inference`, `Embedding`, and `NotReady` variants using `thiserror` in `local-llm/src/error.rs`
- [ ] T005 [P] Implement `ProgressEvent` enum (`DownloadProgress`, `DownloadComplete`, `LoadingProgress`, `LoadingComplete`) and `ProgressCallbackFn` type alias in `local-llm/src/progress.rs`
- [ ] T006 [P] Implement `ModelConfig` struct (`repo_id`, `filename`, `context_length`, `chat_template`) with `LOCAL_CONTEXT_LENGTH` env var override logic in `local-llm/src/preset.rs`
- [ ] T007 Implement `ModelPreset` enum (`SmolLM3_3B`, `EmbeddingGemma300M`) with `config()` and `all()` methods in `local-llm/src/preset.rs`
- [ ] T008 [P] Implement `ModelState` enum (`Unloaded`, `Downloading`, `Loading`, `Ready`, `Failed(String)`) in `local-llm/src/model.rs`
- [ ] T009 Add unit tests for `ModelPreset::config()` verifying correct `repo_id`, `filename`, and `context_length` for each preset in `local-llm/src/preset.rs` (inline tests)
- [ ] T010 Add unit tests for `ModelConfig` verifying `LOCAL_CONTEXT_LENGTH` env var override in `local-llm/src/preset.rs` (inline tests)
- [ ] T011 Add unit tests for `LocalModelError` Display formatting and variant construction in `local-llm/src/error.rs` (inline tests)

**Checkpoint**: Foundation ready — all shared types are defined and tested. User story implementation can begin.

---

## Phase 3: User Story 1 — Run Inference Locally Without Cloud Credentials (Priority: P1)

**Goal**: A developer can configure a local model, send a prompt, and receive streaming text tokens entirely on-device, without any cloud API keys or network calls to cloud providers.

**Independent Test**: Configure a local model, send a prompt, verify that text tokens stream back incrementally. Live tests require ~2.1 GB model download (`--ignored`).

### Tests for User Story 1 (Red-Green-Refactor: write tests first)

- [ ] T012 [P] [US1] Add unit tests for `LocalModel` state transitions: `Unloaded` initial state, `with_progress` before/after `ensure_ready`, `send_chat_request` on unloaded model returns `NotReady` in `local-llm/src/model.rs` (inline tests)
- [ ] T013 [P] [US1] Add unit tests for message conversion: system prompt positioning, user/assistant mapping, tool call/result serialization, CustomMessage filtering in `local-llm/src/convert.rs` (inline tests)
- [ ] T014 [P] [US1] Add unit tests for `LocalStreamFn` think-tag parsing and event emission in `local-llm/src/stream.rs` (inline tests)
- [ ] T015 [P] [US1] Add unit tests for context truncation logic in `local-llm/src/stream.rs` (inline tests)

### Implementation for User Story 1

- [ ] T016 [US1] Implement `LocalModel::new(config)` constructor creating a model in `Unloaded` state with `Arc<Mutex<ModelState>>` in `local-llm/src/model.rs`
- [ ] T017 [US1] Implement `LocalModel::from_preset(preset)` as `Self::new(preset.config())` in `local-llm/src/model.rs`
- [ ] T018 [US1] Implement `LocalModel::with_progress(callback)` that attaches a `ProgressCallbackFn` (must be called before `ensure_ready`, returns `Err` if called after) in `local-llm/src/model.rs`
- [ ] T019 [US1] Implement `LocalModel::ensure_ready()` — download model via `hf-hub` if not cached, load via `mistralrs` GGUF pipeline, transition through `Unloaded → Downloading → Loading → Ready` (or `Failed`), idempotent if already `Ready`, re-attempt if `Failed`. Integrity verification delegated to hf-hub ETag/SHA (FR-009) in `local-llm/src/model.rs`
- [ ] T020 [US1] Implement `LocalModel::send_chat_request()` — run inference on the loaded `MistralRs` runner, return `NotReady` error if model not loaded, in `local-llm/src/model.rs`
- [ ] T021 [US1] Implement message conversion from `LlmMessage` to local model format — system prompts, user/assistant messages, tool calls, tool results, filter out `CustomMessage` — in `local-llm/src/convert.rs`
- [ ] T022 [US1] Implement `LocalStreamFn::new(model)` constructor in `local-llm/src/stream.rs`
- [ ] T023 [US1] Implement `StreamFn` trait for `LocalStreamFn` — call `ensure_ready()` on first invocation, convert `LlmMessage` list to local format via `convert.rs`, run inference, wrap output into `AssistantMessageEvent` stream (`Start → ContentBlockStart → ContentBlockDelta → ContentBlockEnd → Done`), cost always zero in `local-llm/src/stream.rs`
- [ ] T024 [US1] Implement `<think>` tag parsing in `LocalStreamFn` — detect `<think>`/`</think>` in token stream and emit `ThinkingStart`/`ThinkingDelta`/`ThinkingEnd` events in `local-llm/src/stream.rs`
- [ ] T025 [US1] Implement silent context truncation in `LocalStreamFn` — when input exceeds `context_length`, keep most recent messages to fit budget in `local-llm/src/stream.rs`
- [ ] T026 [US1] Implement live inference test: load `SmolLM3_3B` preset, send a prompt, verify streaming tokens arrive in `local-llm/tests/local_live.rs` (`#[ignore]`)

**Checkpoint**: User Story 1 complete — local inference works end-to-end with streaming tokens, progress reporting, and automatic download/caching.

---

## Phase 4: User Story 2 — Track Model Download and Loading Progress (Priority: P1)

**Goal**: The system reports download progress (bytes transferred, percentage) and loading progress so developers know the system is working, not stuck.

**Independent Test**: Trigger a model download and verify that progress callbacks fire with increasing percentages until completion.

### Tests for User Story 2 (Red-Green-Refactor: write tests first)

- [ ] T027 [US2] Add unit tests verifying progress callback is invoked with correct event variants and that `DownloadComplete`/`LoadingComplete` each fire exactly once in `local-llm/src/model.rs` (inline tests using mock callback)

### Implementation for User Story 2

- [ ] T028 [US2] Wire `ProgressCallbackFn` into `ensure_ready()` download phase — emit `DownloadProgress` at least every 1% of total bytes and `DownloadComplete` on success in `local-llm/src/model.rs`
- [ ] T029 [US2] Wire `ProgressCallbackFn` into `ensure_ready()` loading phase — emit `LoadingProgress` with status messages and `LoadingComplete` when ready in `local-llm/src/model.rs`
- [ ] T030 [US2] Handle download interruption — ensure clean error propagation via `Download` variant when network fails mid-download (resume behavior delegated to hf-hub) in `local-llm/src/model.rs`
- [ ] T031 [US2] Add live progress test: download a model with progress callback, verify `DownloadProgress` events have increasing `bytes_downloaded` in `local-llm/tests/local_live.rs` (`#[ignore]`)

**Checkpoint**: User Story 2 complete — progress reporting works during download and loading phases.

---

## Phase 5: User Story 3 — Embed Text for Similarity Comparisons (Priority: P2)

**Goal**: A developer can compute vector embeddings for text passages locally, enabling similarity search, clustering, or RAG workflows without cloud API calls.

**Independent Test**: Embed two semantically similar texts and two dissimilar texts, verify the similar pair has higher cosine similarity.

### Tests for User Story 3 (Red-Green-Refactor: write tests first)

- [ ] T032 [US3] Add unit tests for `EmbeddingModel` state transitions, constructor behavior, embed error on max length exceeded, and valid vector for empty input in `local-llm/src/embedding.rs` (inline tests)

### Implementation for User Story 3

- [ ] T033 [US3] Implement `EmbeddingModel::new(config)` and `EmbeddingModel::from_preset(preset)` constructors in `local-llm/src/embedding.rs`
- [ ] T034 [US3] Implement `EmbeddingModel::with_progress(callback)` in `local-llm/src/embedding.rs`
- [ ] T035 [US3] Implement `EmbeddingModel::ensure_ready()` — download and load the embedding model via `mistralrs`, same lifecycle as `LocalModel` in `local-llm/src/embedding.rs`
- [ ] T036 [US3] Implement `EmbeddingModel::embed(text)` — compute a fixed-dimensional vector, return `Embedding` error if input exceeds max length, return valid vector for empty input in `local-llm/src/embedding.rs`
- [ ] T037 [US3] Implement `EmbeddingModel::embed_batch(texts)` — batch embedding, fail on first invalid input in `local-llm/src/embedding.rs`
- [ ] T038 [US3] Add live embedding test: load `EmbeddingGemma300M`, embed similar/dissimilar text pairs, verify cosine similarity ordering in `local-llm/tests/embedding_live.rs` (`#[ignore]`)
- [ ] T039 [US3] Add live embedding test: verify empty input returns a valid vector (not an error) in `local-llm/tests/embedding_live.rs` (`#[ignore]`)
- [ ] T040 [US3] Add live embedding test: verify input exceeding max length returns `Embedding` error in `local-llm/tests/embedding_live.rs` (`#[ignore]`)

**Checkpoint**: User Story 3 complete — local embeddings work for similarity comparisons.

---

## Phase 6: User Story 4 — Use Local Model Presets (Priority: P2)

**Goal**: A developer selects a model by preset name without manual configuration. Presets bundle repo ID, filename, quantization, and context length.

**Independent Test**: Select a preset by name and verify the model loads with correct configuration.

### Implementation for User Story 4

- [ ] T041 [US4] Verify `ModelPreset::SmolLM3_3B` configures correct `repo_id` (`HuggingFaceTB/SmolLM3-3B-GGUF`), `filename` (`smollm3-3b-q4_k_m.gguf`), and `context_length` (8192) in `local-llm/src/preset.rs`
- [ ] T042 [US4] Verify `ModelPreset::EmbeddingGemma300M` configures correct `repo_id` and `filename` for the embedding model in `local-llm/src/preset.rs`
- [ ] T043 [US4] Verify `ModelPreset::all()` returns a static slice containing all preset variants in `local-llm/src/preset.rs`
- [ ] T044 [US4] Add unit test: `LocalModel::from_preset(SmolLM3_3B)` creates model with correct config in `local-llm/src/model.rs` (inline test)
- [ ] T045 [US4] Add unit test: `EmbeddingModel::from_preset(EmbeddingGemma300M)` creates model with correct config in `local-llm/src/embedding.rs` (inline test)

**Checkpoint**: User Story 4 complete — presets provide zero-config model setup.

---

## Phase 7: User Story 5 — Convert Agent Messages to Local Model Format (Priority: P3)

**Goal**: Standard agent messages (system, user, assistant, tool calls, tool results) are automatically converted to the local model's expected format with correct special tokens.

**Independent Test**: Convert a representative set of agent messages (including tool calls) and verify the output matches the expected local format.

### Implementation for User Story 5

- [ ] T046 [US5] Verify system prompt conversion places the system message in the correct position for the local model format in `local-llm/src/convert.rs`
- [ ] T047 [US5] Verify tool call messages are serialized correctly in the local format (function name, arguments JSON) in `local-llm/src/convert.rs`
- [ ] T048 [US5] Verify tool result messages are serialized correctly in the local format (tool output text) in `local-llm/src/convert.rs`
- [ ] T049 [US5] Verify `CustomMessage` variants are filtered out and never sent to the local model in `local-llm/src/convert.rs`
- [ ] T050 [US5] Add unit test: round-trip conversion of a full conversation (system + user + assistant + tool call + tool result) in `local-llm/src/convert.rs` (inline test)

**Checkpoint**: User Story 5 complete — message conversion handles all agent message types.

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Edge case handling, documentation, and final validation

- [ ] T051 [P] Add error handling for disk-full during download: verify `Download` error propagates OS I/O error in `local-llm/src/model.rs`
- [ ] T052 [P] Add error handling for corrupted GGUF file: verify `Loading` error covers parse failures in `local-llm/src/model.rs`
- [ ] T053 [P] Add error handling for out-of-memory during load: verify `Loading` error covers OOM in `local-llm/src/model.rs`
- [ ] T054 Verify `local-llm/CLAUDE.md` documents lessons learned, active technologies, and test commands
- [ ] T055 Run `cargo build -p swink-agent-local-llm` and verify clean compilation with zero warnings
- [ ] T056 Run `cargo test -p swink-agent-local-llm` and verify all non-ignored tests pass
- [ ] T057 Run `cargo clippy -p swink-agent-local-llm -- -D warnings` and verify zero clippy warnings
- [ ] T058 Validate quickstart.md code examples compile and match the implemented public API

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational — core inference path
- **US2 (Phase 4)**: Depends on US1 T015 (`ensure_ready` must exist to wire progress into)
- **US3 (Phase 5)**: Depends on Foundational only — independent of US1/US2
- **US4 (Phase 6)**: Depends on Foundational only — preset verification
- **US5 (Phase 7)**: Depends on Foundational only — message conversion verification
- **Polish (Phase 8)**: Depends on all user stories being complete

### User Story Dependencies

- **US1 (P1)**: Depends on Phase 2 — core inference, conversion, streaming
- **US2 (P1)**: Depends on US1 (T015 `ensure_ready`) — adds progress wiring
- **US3 (P2)**: Depends on Phase 2 only — independent embedding model
- **US4 (P2)**: Depends on Phase 2 only — preset correctness
- **US5 (P3)**: Depends on Phase 2 only — conversion correctness

### Within Each User Story

- Models/types before services
- Core implementation before tests
- Unit tests before live tests

### Parallel Opportunities

- T005, T006, T008 can run in parallel (different files)
- T009, T010, T011 can run in parallel (inline tests in different files)
- US3, US4, US5 can all run in parallel after Phase 2 (independent concerns)
- T051, T052, T053 can run in parallel (different error scenarios)

---

## Parallel Example: User Story 1

```text
# Sequential core path:
T012 → T013 → T014 → T015 → T016 (LocalModel lifecycle)

# Then parallel:
T017 (convert.rs) | T018 + T019 (stream.rs) | T020 (think tags) | T021 (truncation)

# Then parallel tests:
T022 (convert tests) | T023 (model tests) | T024 (think tests) | T025 (truncation tests)

# Finally:
T026 (live test)
```

## Parallel Example: After Phase 2

```text
# These three user stories can proceed in parallel:
US3 (T032–T040, embedding) | US4 (T041–T045, presets) | US5 (T046–T050, conversion)
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational
3. Complete Phase 3: User Story 1 (local inference)
4. **STOP and VALIDATE**: `cargo test -p swink-agent-local-llm` passes, live inference test works
5. This is the minimum viable local LLM experience

### Incremental Delivery

1. Setup + Foundational → Foundation ready
2. US1 → Local inference works → MVP!
3. US2 → Progress reporting during download/load
4. US3 → Embedding support
5. US4 → Preset validation
6. US5 → Message conversion validation
7. Polish → Edge cases, docs, final checks

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Live tests (`#[ignore]`) require ~2.1 GB model download for inference, separate download for embeddings
- Source files already exist in `local-llm/src/` — tasks modify/complete existing files, not create from scratch
- The `mistralrs` and `hf-hub` crates handle the heavy lifting; implementation wraps their APIs
