# Tasks: Adapter Shared Infrastructure

**Input**: Design documents from `/specs/011-adapter-shared-infra/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md

**Tests**: Included — the spec calls for unit tests per module and the quickstart lists test commands.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Verify project structure, dependencies, and crate wiring before implementing modules

- [x] T001 Verify adapters crate dependencies in `adapters/Cargo.toml` include `reqwest`, `futures`, `bytes`, `serde_json`, `thiserror`, `tokio` and path dep on `swink-agent`
- [x] T002 Verify `adapters/src/lib.rs` has `#![forbid(unsafe_code)]` and module declarations for `convert`, `classify`, `sse`, `remote_presets`
- [x] T003 Verify `src/convert.rs` exists in core (`swink-agent`) with the `MessageConverter` trait definition site

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core `MessageConverter` trait in `swink-agent` core — all user stories depend on this existing in core before adapters can re-export or use it

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 Define the `MessageConverter` trait in `src/convert.rs` (core) with methods for system prompt extraction, message-level conversion, and content-block-level conversion per the public API contract
- [x] T005 Implement the generic `convert_messages<C: MessageConverter>()` driver function in `src/convert.rs` (core) that iterates agent messages and delegates to the converter
- [x] T006 Implement `extract_tool_schemas()` function in `src/convert.rs` (core) that extracts JSON schemas from `Arc<dyn AgentTool>` for provider tool-use payloads
- [x] T007 Re-export `MessageConverter`, `convert_messages`, and `extract_tool_schemas` from `src/lib.rs` (core) public API

**Checkpoint**: Foundation ready — `swink-agent` core exposes the conversion trait; user story implementation can now begin in parallel

---

## Phase 3: User Story 1 — Convert Messages to Provider Format (Priority: P1) MVP

**Goal**: Adapter developers can implement `MessageConverter` for their provider and use `convert_messages()` to convert agent messages to provider-specific JSON.

**Independent Test**: Implement the conversion trait for a mock provider format and verify all message types (user, assistant, tool result) and content block types (text, thinking, tool call, image) convert correctly.

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T008 [P] [US1] Write unit tests for `MessageConverter` trait with a mock converter verifying all message types convert correctly in `src/convert.rs` (core, `#[cfg(test)]` module)

### Implementation for User Story 1

- [x] T009 [US1] Implement `adapters/src/convert.rs` re-exporting `MessageConverter`, `convert_messages`, and `extract_tool_schemas` from core
- [x] T010 [US1] Add `convert` module re-exports to `adapters/src/lib.rs` public API

**Checkpoint**: User Story 1 complete — adapters can import `MessageConverter` from either core or the adapters crate and convert messages generically

---

## Phase 4: User Story 2 — Classify HTTP Errors Consistently (Priority: P1)

**Goal**: Adapter developers use `classify_http_status()` and `classify_with_overrides()` to map HTTP status codes to agent error types consistently across all providers.

**Independent Test**: Pass various HTTP status codes (200, 401, 403, 429, 500, 502, 503) and verify correct `HttpErrorKind` variants are returned.

### Tests for User Story 2

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T011 [P] [US2] Write unit tests for `classify_http_status` verifying: 429 -> Throttled, 401/403 -> Auth, 500-599 -> Network, 200 -> None in `adapters/src/classify.rs` (`#[cfg(test)]` module). Note: connection-level timeouts are NOT classified here — they are mapped to `NetworkError` at the adapter level by reqwest error handling (see spec edge cases section).
- [x] T012 [P] [US2] Write unit tests for `classify_with_overrides` verifying overrides take precedence over defaults in `adapters/src/classify.rs` (`#[cfg(test)]` module)

### Implementation for User Story 2

- [x] T013 [P] [US2] Define `HttpErrorKind` enum with `Auth`, `Throttled`, `Network` variants (derives: `Debug, Clone, PartialEq, Eq`) in `adapters/src/classify.rs`
- [x] T014 [US2] Implement `classify_http_status` as `const fn(u16) -> Option<HttpErrorKind>` with default mappings (429=Throttled, 401/403=Auth, 500-599=Network) in `adapters/src/classify.rs`
- [x] T015 [US2] Implement `classify_with_overrides` function that checks override slice before falling back to `classify_http_status` in `adapters/src/classify.rs`
- [x] T016 [US2] Add `classify` module re-exports to `adapters/src/lib.rs` public API

**Checkpoint**: User Story 2 complete — adapters can classify HTTP errors consistently with optional provider overrides

---

## Phase 5: User Story 3 — Parse SSE Streams (Priority: P1)

**Goal**: Adapter developers use `SseStreamParser` and `sse_data_lines()` to consume Server-Sent Events from provider streaming endpoints without duplicating protocol handling.

**Independent Test**: Feed raw SSE text bytes to the parser and verify correctly parsed `SseLine` variants are produced, including handling of partial chunks, comments, terminal `[DONE]`, and the `sse_data_lines` combinator filtering.

### Tests for User Story 3

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T017 [P] [US3] Write unit tests for `SseStreamParser::feed()` verifying: data lines, event lines, empty lines, comment skipping, `data: [DONE]` -> Done variant, and multi-line `data:` field concatenation (successive `data:` lines joined with `\n` per SSE spec) in `adapters/src/sse.rs` (`#[cfg(test)]` module)
- [x] T018 [P] [US3] Write unit tests for `SseStreamParser` partial chunk buffering: split a complete SSE event across multiple `feed()` calls and verify correct reassembly in `adapters/src/sse.rs` (`#[cfg(test)]` module)
- [x] T019 [P] [US3] Write unit tests for `SseStreamParser::flush()` verifying remaining buffer is drained at stream end in `adapters/src/sse.rs` (`#[cfg(test)]` module)

### Implementation for User Story 3

- [x] T020 [P] [US3] Define `SseLine` enum with `Event(String)`, `Data(String)`, `Done`, `Empty` variants (derives: `Debug, PartialEq, Eq`) in `adapters/src/sse.rs`
- [x] T021 [US3] Implement `SseStreamParser` struct with `buffer: String` field, `new()` as `const fn`, and `Default` impl delegating to `new()` in `adapters/src/sse.rs`
- [x] T022 [US3] Implement `SseStreamParser::feed(&mut self, &[u8]) -> Vec<SseLine>` that accumulates bytes, splits on newlines, parses prefixes (`data:`, `event:`, `:` comments), concatenates successive `data:` lines with `\n` per SSE spec (FR-006), and yields complete lines in `adapters/src/sse.rs`
- [x] T023 [US3] Implement `SseStreamParser::flush(&mut self) -> Vec<SseLine>` that drains the remaining buffer in `adapters/src/sse.rs`
- [x] T024 [US3] Implement `sse_data_lines()` stream combinator that wraps a `reqwest` byte stream with `SseStreamParser` and filters to only `Data` and `Done` variants in `adapters/src/sse.rs`
- [x] T025 [US3] Add `sse` module re-exports to `adapters/src/lib.rs` public API

**Checkpoint**: User Story 3 complete — adapters can parse SSE streams using shared infrastructure

---

## Phase 6: User Story 4 — Construct Connections from Catalog Presets (Priority: P2)

**Goal**: Developers select a model from the catalog and the system constructs a fully configured remote connection with the appropriate streaming function, eliminating manual wiring.

**Independent Test**: Select a catalog preset key and verify a correctly configured connection is produced (or appropriate error for missing credentials/unknown preset).

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T026 [P] [US4] Write unit tests for `RemotePresetKey` construction and equality in `adapters/src/remote_presets.rs` (`#[cfg(test)]` module)
- [x] T027 [P] [US4] Write unit tests for `remote_presets()` listing: all presets returned when filter is None, only matching provider when filtered in `adapters/src/remote_presets.rs` (`#[cfg(test)]` module)
- [x] T028 [P] [US4] Write unit tests for `build_remote_connection` error paths: `UnknownPreset`, `MissingCredential` in `adapters/src/remote_presets.rs` (`#[cfg(test)]` module)

### Implementation for User Story 4

- [x] T029 [P] [US4] Define `RemotePresetKey` struct with `provider_key: &'static str`, `preset_id: &'static str` (derives: `Debug, Clone, Copy, PartialEq, Eq, Hash`) and `const fn new()` in `adapters/src/remote_presets.rs`
- [x] T030 [US4] Define `RemoteModelConnectionError` enum with `UnknownPreset`, `NotRemotePreset`, `MissingCredential`, `MissingBaseUrl`, `MissingRegion`, `MissingAwsCredentials` variants (derives: `Debug, thiserror::Error, PartialEq, Eq`) in `adapters/src/remote_presets.rs`
- [x] T031 [US4] Implement `remote_preset_keys` module with nested provider sub-modules (`anthropic`, `openai`, `google`, `azure`, `xai`, `mistral`, `bedrock`) containing compile-time `RemotePresetKey` constants in `adapters/src/remote_presets.rs`
- [x] T032 [US4] Implement `remote_presets(provider_key: Option<&str>) -> Vec<CatalogPreset>` that lists all remote presets, optionally filtered by provider, using the model catalog in `adapters/src/remote_presets.rs`
- [x] T033 [US4] Implement `preset(model_id: &str) -> Option<CatalogPreset>` that finds a preset by model_id string in `adapters/src/remote_presets.rs`
- [x] T034 [US4] Implement `build_remote_connection(key: RemotePresetKey) -> Result<ModelConnection, RemoteModelConnectionError>` that resolves credentials from environment variables and constructs the appropriate `StreamFn` in `adapters/src/remote_presets.rs`
- [x] T035 [US4] Add `remote_presets` module re-exports (including `remote_preset_keys`, `build_remote_connection`, `preset`, `remote_presets`, `RemotePresetKey`, `RemoteModelConnectionError`) to `adapters/src/lib.rs` public API

**Checkpoint**: User Story 4 complete — catalog presets resolve to fully configured remote connections

---

## Phase 7: User Story 5 — Prompt Caching Strategy (Priority: P2) — I6

**Goal**: Provider-agnostic caching configuration via `CacheStrategy` enum flowing through `StreamOptions`

**Independent Test**: Configure `CacheStrategy::Auto`, call adapter's `apply_cache_strategy()`, verify provider-specific markers injected

### Tests for User Story 5

- [x] T041 [P] [US5] Unit test `cache_strategy_none_no_markers` in `adapters/tests/`: verify no cache markers when `CacheStrategy::None`
- [x] T042 [P] [US5] Unit test `cache_strategy_auto_anthropic_markers` in `adapters/tests/`: verify Anthropic adapter injects `cache_control` blocks on system prompt and tool definitions when `Auto`
- [x] T043 [P] [US5] Unit test `cache_strategy_ignored_by_unsupporting_adapter` in `adapters/tests/`: verify an adapter without caching support ignores the strategy silently

### Implementation for User Story 5

- [x] T044 [US5] Add `CacheStrategy` enum (`None`, `Auto`, `Anthropic`, `Google { ttl }`) to `src/stream.rs` in core. Derive `Debug`, `Clone`, `Default` (default = `None`).
- [x] T045 [US5] Add `cache_strategy: CacheStrategy` field to `StreamOptions` in `src/stream.rs` with `#[serde(default)]`.
- [x] T046 [US5] Implement `apply_cache_strategy()` in the Anthropic adapter (`adapters/src/anthropic.rs`): inject `cache_control: { type: "ephemeral" }` on system prompt and tool definitions when `Auto` or `Anthropic`.
- [x] T047 [US5] Verify other adapters (OpenAI, Ollama, Mistral, xAI, Azure) silently ignore `CacheStrategy` (no-op — no code changes needed, just verify).
- [x] T048 [US5] Re-export `CacheStrategy` from `src/lib.rs` (core).

**Checkpoint**: US5 complete — caching strategy flows from configuration to adapters

---

## Phase 8: User Story 6 — Proxy Streaming Mode (Priority: P3) — N3

**Goal**: Raw SSE byte relay for gateway deployments via `ProxyStreamFn`

**Independent Test**: Configure `ProxyStreamFn`, send request, verify consumer receives raw SSE bytes

### Tests for User Story 6

- [x] T049 [P] [US6] Integration test `proxy_stream_raw_bytes` in `adapters/tests/`: configure `ProxyStreamFn` with a mock HTTP server, verify raw bytes are relayed
- [x] T050 [P] [US6] Integration test `proxy_stream_error_propagated` in `adapters/tests/`: mock HTTP server returns 500, verify error propagated to consumer

### Implementation for User Story 6

- [x] T051 [US6] Implement `ProxyStreamFn` struct in `adapters/src/proxy.rs` (or new `proxy_raw.rs`): `new()` constructor, `stream_raw()` method returning `Stream<Item = Result<Bytes, Error>>`.
- [x] T052 [US6] Re-export `ProxyStreamFn` from `adapters/src/lib.rs`.

**Checkpoint**: US6 complete — raw SSE relay available for gateway use cases

---

## Phase 9: User Story 7 — Raw Provider Payload Callback (Priority: P3) — N4

**Goal**: Optional `on_raw_payload` callback in `StreamOptions` for observing raw SSE data lines

**Independent Test**: Configure callback, send request, verify callback fires with raw data lines

### Tests for User Story 7

- [x] T053 [P] [US7] Unit test `on_raw_payload_fires_for_each_line` in `adapters/tests/`: configure callback, feed SSE data, verify callback called for each data line
- [x] T054 [P] [US7] Unit test `on_raw_payload_none_no_overhead` in `adapters/tests/`: verify no overhead when callback is `None`
- [x] T055 [P] [US7] Unit test `on_raw_payload_panic_caught` in `adapters/tests/`: configure panicking callback, verify stream continues

### Implementation for User Story 7

- [x] T056 [US7] Add `OnRawPayload` type alias and `on_raw_payload: Option<OnRawPayload>` field to `StreamOptions` in `src/stream.rs` (core).
- [x] T057 [US7] Integrate `on_raw_payload` invocation into `sse_data_lines()` in `adapters/src/sse.rs`: call callback with raw data line string before yielding `SseLine::Data`. Wrap in `catch_unwind`.
- [x] T058 [US7] Re-export `OnRawPayload` from `src/lib.rs` (core).

**Checkpoint**: US7 complete — raw payload observation available for debugging

---

## Phase 10: Polish & Cross-Cutting Concerns

**Purpose**: Final verification across all modules

- [x] T036 [P] Verify all public types are re-exported from `adapters/src/lib.rs` per the public API contract in `specs/011-adapter-shared-infra/contracts/public-api.md`
- [x] T037 Run `cargo build -p swink-agent-adapters` and `cargo test -p swink-agent-adapters` to verify compilation and all tests pass
- [x] T038 Run `cargo clippy --workspace -- -D warnings` to verify zero clippy warnings
- [x] T039 Run `cargo test --workspace` to verify no regressions in other crates
- [x] T040 Run quickstart.md validation: execute all code examples from `specs/011-adapter-shared-infra/quickstart.md` usage section
- [x] T059 Verify `CacheStrategy`, `OnRawPayload`, and `ProxyStreamFn` are re-exported from their respective crate roots
- [x] T060 Run `cargo build --workspace` and verify zero compilation errors with new types
- [x] T061 Run `cargo test --workspace` and verify all new tests pass
- [x] T062 Run `cargo clippy --workspace -- -D warnings` and fix any warnings
- [x] T063 Validate new quickstart.md examples (caching, raw payload, proxy) match actual API

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational (Phase 2) — no other story dependencies
- **US2 (Phase 4)**: Depends on Foundational (Phase 2) — independent of US1, US3, US4
- **US3 (Phase 5)**: Depends on Foundational (Phase 2) — independent of US1, US2, US4
- **US4 (Phase 6)**: Depends on Foundational (Phase 2) — may reference types from US1/US2/US3 but is independently testable
- **US5 Caching (Phase 7)**: Depends on Phase 2. Modifies core `StreamOptions` and Anthropic adapter.
- **US6 Proxy (Phase 8)**: Depends on Phase 2. Adds new struct in adapters. Independent of US5/US7.
- **US7 Raw Payload (Phase 9)**: Depends on Phase 2 and US3 (SSE parsing). Modifies `sse_data_lines()`.
- **Polish (Phase 10)**: Depends on all user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Phase 2 — no dependencies on other stories
- **User Story 2 (P1)**: Can start after Phase 2 — no dependencies on other stories
- **User Story 3 (P1)**: Can start after Phase 2 — no dependencies on other stories
- **User Story 4 (P2)**: Can start after Phase 2 — uses types from US1/US3 indirectly (via adapters) but is independently implementable

### Within Each User Story

- Tests MUST be written and FAIL before implementation
- Type definitions before functions
- Core functions before convenience combinators
- Module implementation before lib.rs re-exports

### Parallel Opportunities

- US1, US2, US3 are fully independent and can all start in parallel after Phase 2
- US5, US6 can proceed in parallel after Phase 2 (separate files, no cross-deps)
- US7 depends on US3 (modifies sse_data_lines) but can proceed after US3 is complete
- Within US2: T011, T012 (tests) in parallel; T013 (types) in parallel with tests
- Within US3: T017, T018, T019 (tests) in parallel; T020 (types) in parallel with tests
- Within US4: T026, T027, T028 (tests) in parallel; T029 (types) in parallel with tests
- All [P] tasks within a phase can run simultaneously

---

## Parallel Example: User Story 2

```text
# Launch all tests together:
Task T011: "Unit tests for classify_http_status in adapters/src/classify.rs"
Task T012: "Unit tests for classify_with_overrides in adapters/src/classify.rs"

# Launch type definition in parallel with tests:
Task T013: "Define HttpErrorKind enum in adapters/src/classify.rs"

# Sequential after types defined:
Task T014: "Implement classify_http_status"
Task T015: "Implement classify_with_overrides"
Task T016: "Re-exports in lib.rs"
```

## Parallel Example: User Story 3

```text
# Launch all tests together:
Task T017: "Unit tests for feed()"
Task T018: "Unit tests for partial chunk buffering"
Task T019: "Unit tests for flush()"

# Launch type definition in parallel with tests:
Task T020: "Define SseLine enum"

# Sequential after types:
Task T021: "Implement SseStreamParser struct"
Task T022: "Implement feed()"
Task T023: "Implement flush()"
Task T024: "Implement sse_data_lines() combinator"
Task T025: "Re-exports in lib.rs"
```

---

## Implementation Strategy

### MVP First (User Stories 1-3)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks all stories)
3. Complete Phase 3: User Story 1 (message conversion re-exports)
4. Complete Phase 4: User Story 2 (HTTP error classification)
5. Complete Phase 5: User Story 3 (SSE parsing)
6. **STOP and VALIDATE**: All P1 stories functional and testable independently
7. Run `cargo test -p swink-agent-adapters` — all pass

### Incremental Delivery

1. Setup + Foundational -> Core conversion trait ready
2. Add US1 -> Conversion re-exports available -> Adapters can start implementing converters
3. Add US2 -> Error classification available -> Adapters get consistent error handling
4. Add US3 -> SSE parsing available -> Streaming adapters unblocked
5. Add US4 -> Catalog-driven connections -> Full preset system operational
6. Each story adds value without breaking previous stories

---

## Notes

- [P] tasks = different files, no dependencies
- [Story] label maps task to specific user story for traceability
- Each user story is independently completable and testable
- Verify tests fail before implementing
- Commit after each task or logical group
- Stop at any checkpoint to validate story independently
- US5 (Caching) adds `CacheStrategy` to core `StreamOptions` and modifies Anthropic adapter
- US6 (Proxy) adds `ProxyStreamFn` — new struct in adapters crate
- US7 (Raw Payload) adds `OnRawPayload` to core `StreamOptions` and modifies `sse_data_lines()`
