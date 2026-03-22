# Tasks: Adapter: Ollama

**Input**: Design documents from `/specs/014-adapter-ollama/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md
**Depends on**: 011-adapter-shared-infra (shared `MessageConverter`, `StreamFinalize`, `extract_tool_schemas`)

**Tests**: Included — unit tests in `adapters/src/ollama.rs` (`#[cfg(test)]` module) and live integration tests in `adapters/tests/ollama_live.rs` (`#[ignore]`).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3, US4)
- Include exact file paths in descriptions

---

## Phase 1: Setup & Scaffolding

**Purpose**: Verify project structure, dependencies, and module wiring before implementing the adapter

- [ ] T001 Verify `adapters/Cargo.toml` includes all required dependencies: `reqwest`, `futures`, `bytes`, `serde`, `serde_json`, `tokio`, `tokio-util`, `tracing`, `uuid`, and path dep on `swink-agent`
- [ ] T002 Add `mod ollama;` declaration to `adapters/src/lib.rs`
- [ ] T003 Add `pub use ollama::OllamaStreamFn;` re-export to `adapters/src/lib.rs`
- [ ] T004 Create `adapters/src/ollama.rs` with module-level doc comment describing Ollama NDJSON adapter

**Checkpoint**: Module wiring complete — `OllamaStreamFn` is importable from `swink_agent_adapters`

---

## Phase 2: Request Types (Blocking Prerequisites)

**Purpose**: Define the serializable request types used to construct the Ollama API request body. All user stories depend on these types.

**CRITICAL**: No user story work can begin until this phase is complete

- [ ] T005 [P] Define `OllamaMessage` struct in `adapters/src/ollama.rs` with `role: String`, `content: String`, `tool_calls: Option<Vec<OllamaToolCall>>` (skip_serializing_if None)
- [ ] T006 [P] Define `OllamaToolCall` struct in `adapters/src/ollama.rs` with `function: OllamaFunctionCall`
- [ ] T007 [P] Define `OllamaFunctionCall` struct in `adapters/src/ollama.rs` with `name: String`, `arguments: Value`
- [ ] T008 [P] Define `OllamaTool` struct in `adapters/src/ollama.rs` with `type` (as `r#type: String`), `function: OllamaToolDef`
- [ ] T009 [P] Define `OllamaToolDef` struct in `adapters/src/ollama.rs` with `name: String`, `description: String`, `parameters: Value`
- [ ] T010 [P] Define `OllamaOptions` struct in `adapters/src/ollama.rs` with `temperature: Option<f64>` (skip_if None), `num_predict: Option<u64>` (skip_if None)
- [ ] T011 Define `OllamaChatRequest` struct in `adapters/src/ollama.rs` with fields: `model`, `messages: Vec<OllamaMessage>`, `stream: bool`, `options: Option<OllamaOptions>` (skip_if None), `tools: Vec<OllamaTool>` (skip_if empty), `think: Option<bool>` (skip_if None)

**Checkpoint**: Request types ready — all serializable types compile and produce JSON matching the Ollama API format

---

## Phase 3: Response Types (Deserialization)

**Purpose**: Define the NDJSON response types with lenient `#[serde(default)]` annotations for robust parsing

- [ ] T012 [P] Define `OllamaChatChunk` struct in `adapters/src/ollama.rs` with `message: OllamaResponseMessage`, `done: bool`, `done_reason: Option<String>` (default), `prompt_eval_count: Option<u64>` (default), `eval_count: Option<u64>` (default)
- [ ] T013 [P] Define `OllamaResponseMessage` struct in `adapters/src/ollama.rs` with `content: String` (default), `thinking: Option<String>` (default), `tool_calls: Option<Vec<OllamaResponseToolCall>>` (default)
- [ ] T014 [P] Define `OllamaResponseToolCall` struct in `adapters/src/ollama.rs` with `function: OllamaResponseFunction`
- [ ] T015 [P] Define `OllamaResponseFunction` struct in `adapters/src/ollama.rs` with `name: String`, `arguments: Value`

**Checkpoint**: Response types ready — NDJSON chunks can be deserialized with missing/optional fields handled leniently

---

## Phase 4: Core Struct & StreamFn Trait

**Purpose**: Define the public `OllamaStreamFn` struct and implement `StreamFn`

- [ ] T016 Define `OllamaStreamFn` struct in `adapters/src/ollama.rs` with `base_url: String` and `client: reqwest::Client` fields
- [ ] T017 Implement `OllamaStreamFn::new(base_url)` constructor accepting `impl Into<String>`, with `#[must_use]`, creating a default `reqwest::Client`
- [ ] T018 Implement `Debug` for `OllamaStreamFn` that shows `base_url` and uses `finish_non_exhaustive()` (no api_key to redact — Ollama has no auth)
- [ ] T019 Implement `StreamFn` for `OllamaStreamFn` with `stream()` method delegating to `ollama_stream()` helper
- [ ] T020 Add compile-time `Send + Sync` assertion for `OllamaStreamFn` via `const` block

**Checkpoint**: Public API ready — `OllamaStreamFn` compiles, is `Send + Sync`, and implements `StreamFn`

---

## Phase 5: MessageConverter & NDJSON Parser

**Purpose**: Implement message conversion from agent types to Ollama wire format and the custom NDJSON line parser

- [ ] T021 Implement `MessageConverter for OllamaConverter` in `adapters/src/ollama.rs`: `system_message()` returns system-role `OllamaMessage`, `user_message()` extracts text content, `assistant_message()` maps `ContentBlock::Text` to content string and `ContentBlock::ToolCall` to `tool_calls` array, `tool_result_message()` maps to tool-role with text content
- [ ] T022 Implement `ndjson_lines()` function in `adapters/src/ollama.rs` that converts a `bytes_stream()` into a `Stream<Item = String>` by buffering incoming bytes, splitting on newline boundaries (handling `\r\n` and `\n`), skipping empty lines, and flushing remaining buffer on stream end. Uses zero-copy UTF-8 conversion via `std::str::from_utf8` with `from_utf8_lossy` fallback.

**Checkpoint**: Message conversion and NDJSON parsing ready — agent messages convert to Ollama wire format, and byte streams produce complete JSON lines

---

## Phase 6: StreamState & StreamFinalize

**Purpose**: Define the stream state machine and implement the `StreamFinalize` trait for clean block closure

- [ ] T023 Define `StreamState` struct in `adapters/src/ollama.rs` with `text_started: bool`, `thinking_started: bool`, `content_index: usize`, `tool_calls_started: HashSet<String>`
- [ ] T024 Implement `StreamFinalize` for `StreamState` via `drain_open_blocks()` that drains open thinking blocks first (with `signature: None`), then open text blocks, incrementing `content_index` for each

**Checkpoint**: Stream state ready — `StreamState` tracks open blocks and can drain them for finalization

---

## Phase 7: User Story 1 — Stream Text Responses from Ollama (Priority: P1) MVP

**Goal**: Stream text responses incrementally from the Ollama chat endpoint via NDJSON, emitting text deltas as they arrive.

**Independent Test**: Send a simple prompt to a running Ollama instance and verify text deltas arrive incrementally and the final assembled message is coherent.

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T025 [P] [US1] Write unit test verifying `convert_messages` produces correct `Vec<OllamaMessage>` for a simple user message with system prompt in `adapters/src/ollama.rs` (`#[cfg(test)]` module)
- [ ] T026 [P] [US1] Write unit test verifying `ndjson_lines` correctly splits a byte stream into complete JSON lines, handling partial lines and `\r\n` line endings
- [ ] T027 [P] [US1] Write unit test verifying text content chunks produce `TextStart`, `TextDelta` events with correct `content_index`
- [ ] T028 [P] [US1] Write unit test verifying `done: true` chunk with `done_reason: "stop"` produces `Done` event with `StopReason::Stop`, correct `Usage`, and zero `Cost`
- [ ] T029 [P] [US1] Write unit test verifying empty content chunks are skipped (no events emitted)

### Implementation for User Story 1

- [ ] T030 [US1] Implement `send_request()` async function in `adapters/src/ollama.rs`: construct URL as `{base_url}/api/chat`, convert messages via `convert_messages::<OllamaConverter>()`, convert tools via `extract_tool_schemas()` to `Vec<OllamaTool>`, construct `OllamaChatRequest`, POST with `.json(&body)`, return `reqwest::Response` or `error_network` event
- [ ] T031 [US1] Implement `ollama_stream()` function in `adapters/src/ollama.rs`: call `send_request()`, check HTTP status (non-success maps to `error_network("Ollama HTTP {status}: {body}")`), call `parse_ndjson_stream()` for success, flatten the stream
- [ ] T032 [US1] Implement `parse_ndjson_stream()` in `adapters/src/ollama.rs`: use `ndjson_lines()` to get line stream, `stream::unfold` with `StreamState`, emit `Start` on first iteration, then process each NDJSON line
- [ ] T033 [US1] Handle text content in `parse_ndjson_stream()`: on non-empty `chunk.message.content`, emit `TextStart` (once, tracked by `text_started`), then `TextDelta` with the content string
- [ ] T034 [US1] Handle `done: true` in `parse_ndjson_stream()`: call `finalize_blocks()` to close open blocks, map `done_reason` (`"stop"` → `StopReason::Stop`, `"length"` → `StopReason::Length`, `"tool_calls"` → `StopReason::ToolUse`, other/absent → `StopReason::Stop`), compute `Usage` from `prompt_eval_count`/`eval_count`, emit `Done` with zero `Cost`

**Checkpoint**: User Story 1 complete — text responses stream incrementally from Ollama with correct event ordering, usage tracking, and zero cost

---

## Phase 8: User Story 2 — Stream Tool Call Responses from Ollama (Priority: P1)

**Goal**: Stream tool call blocks using Ollama's native tool-calling protocol, where complete tool calls arrive in a single chunk (not delta-fragmented like OpenAI).

**Independent Test**: Send a prompt with tool definitions and verify tool call events arrive with correct names and parseable arguments.

### Tests for User Story 2

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T035 [P] [US2] Write unit test verifying a chunk with `tool_calls` produces `ToolCallStart`, `ToolCallDelta`, `ToolCallEnd` triplet with correct `name`, generated `id`, and complete JSON `arguments`
- [ ] T036 [P] [US2] Write unit test verifying multiple tool calls in a single chunk produce separate indexed triplets with unique `content_index` values
- [ ] T037 [P] [US2] Write unit test verifying text block is closed (`TextEnd` emitted) before tool call events when the response has both text and tool calls
- [ ] T038 [P] [US2] Write unit test verifying duplicate tool calls (same function name across multiple chunks) are deduplicated via `tool_calls_started` `HashSet`
- [ ] T039 [P] [US2] Write unit test verifying `convert_messages` correctly converts assistant messages with `ContentBlock::ToolCall` to `OllamaMessage` with `tool_calls` array, and `ToolResultMessage` to tool-role message

### Implementation for User Story 2

- [ ] T040 [US2] Implement tool call handling in `parse_ndjson_stream()`: when `chunk.message.tool_calls` is present, close open text block if any (`TextEnd`), then for each tool call — generate `tc_{uuid}` id, check `tool_calls_started` `HashSet` for dedup, emit `ToolCallStart`/`ToolCallDelta`/`ToolCallEnd` triplet with complete arguments, increment `content_index`
- [ ] T041 [US2] Implement tool schema extraction in `send_request()`: convert `extract_tool_schemas()` output to `Vec<OllamaTool>` with `type: "function"`, `name`, `description`, `parameters`

**Checkpoint**: User Story 2 complete — tool calls produce valid events with correct names, IDs, and complete JSON arguments

---

## Phase 9: User Story 3 — Consume NDJSON Streaming Protocol (Priority: P2)

**Goal**: Handle the NDJSON protocol correctly, including thinking content, partial lines, the done flag, and mid-stream errors. Transparent to the developer — same event types as any other adapter.

**Independent Test**: Feed raw NDJSON lines to the parser and verify correctly parsed events, including handling of partial lines, thinking blocks, and the done flag.

### Tests for User Story 3

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T042 [P] [US3] Write unit test verifying `ndjson_lines` correctly handles partial JSON lines split across multiple byte chunks — buffers until newline arrives
- [ ] T043 [P] [US3] Write unit test verifying `ndjson_lines` flushes remaining buffer content on stream end (no trailing newline)
- [ ] T044 [P] [US3] Write unit test verifying thinking content chunks produce `ThinkingStart`, `ThinkingDelta` events with correct `content_index`
- [ ] T045 [P] [US3] Write unit test verifying thinking block is closed (`ThinkingEnd` with `signature: None`) when text content arrives after thinking, with `content_index` incremented
- [ ] T046 [P] [US3] Write unit test verifying empty thinking content (empty string or None) is silently skipped

### Implementation for User Story 3

- [ ] T047 [US3] Implement thinking content handling in `parse_ndjson_stream()`: on non-empty `chunk.message.thinking`, emit `ThinkingStart` (once, tracked by `thinking_started`), then `ThinkingDelta` with the thinking string
- [ ] T048 [US3] Implement thinking-to-text transition in `parse_ndjson_stream()`: when text content arrives and `thinking_started` is true, emit `ThinkingEnd` (with `signature: None`), reset `thinking_started`, increment `content_index` before emitting `TextStart`

**Checkpoint**: User Story 3 complete — NDJSON protocol is parsed correctly with thinking support, partial line buffering, and done flag handling

---

## Phase 10: User Story 4 — Handle Errors from Ollama (Priority: P2)

**Goal**: Classify connection failures, HTTP errors, and parse errors so the agent loop can apply appropriate retry strategies.

**Independent Test**: Simulate error responses and connection failures and verify each maps to the correct error type.

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [ ] T049 [P] [US4] Write unit test verifying connection refused error (reqwest send failure) maps to `error_network("Ollama connection error: ...")` (retryable)
- [ ] T050 [P] [US4] Write unit test verifying non-success HTTP status (e.g., 404 model not found) maps to `error_network("Ollama HTTP {status}: {body}")` (retryable)
- [ ] T051 [P] [US4] Write unit test verifying NDJSON parse error mid-stream (malformed JSON line) maps to `error("Ollama JSON parse error: ...")` (non-retryable), with open blocks finalized
- [ ] T052 [P] [US4] Write unit test verifying unexpected stream end (no `done: true`) maps to `error("Ollama stream ended unexpectedly")` (non-retryable), with open blocks finalized

### Implementation for User Story 4

- [ ] T053 [US4] Implement HTTP error classification in `ollama_stream()`: non-success HTTP status → read response body, emit `error_network("Ollama HTTP {status}: {body}")`
- [ ] T054 [US4] Implement connection error handling in `send_request()`: `reqwest::Error` → `error_network("Ollama connection error: {e}")`
- [ ] T055 [US4] Implement JSON parse error handling in `parse_ndjson_stream()`: `serde_json::from_str` failure → log error, call `finalize_blocks()`, emit `error("Ollama JSON parse error: {e}")`
- [ ] T056 [US4] Implement unexpected stream end handling in `parse_ndjson_stream()`: when `lines.next()` returns `None`, call `finalize_blocks()`, emit `error("Ollama stream ended unexpectedly")`

**Checkpoint**: User Story 4 complete — all Ollama error conditions map to correct agent error types for retry strategy decisions

---

## Phase 11: Cancellation & Edge Cases

**Purpose**: Verify cancellation support and handle edge cases identified in the spec

### Tests for Edge Cases

- [ ] T057 [P] Write unit test verifying cancellation emits finalization events for open blocks followed by an `Error` event with `StopReason::Aborted` and `"operation cancelled"` message
- [ ] T058 [P] Write unit test verifying `StreamFinalize::drain_open_blocks()` correctly drains thinking block first, then text block, in sorted order with correct `content_index` increments
- [ ] T059 [P] Write unit test verifying `StreamFinalize::drain_open_blocks()` is idempotent — second call returns empty
- [ ] T060 [P] Write unit test verifying `convert_messages` skips `CustomMessage` variants in the agent message log (handled by shared `convert_messages`)
- [ ] T061 [P] Write unit test verifying assistant messages with empty text content and only tool calls produce correct `OllamaMessage` with empty `content` and populated `tool_calls`
- [ ] T062 [P] Write unit test verifying `done_reason` mapping: `"tool_calls"` → `StopReason::ToolUse`, `"length"` → `StopReason::Length`, `"stop"` → `StopReason::Stop`, absent/unknown → `StopReason::Stop`

### Implementation for Edge Cases

- [ ] T063 Implement cancellation handling in `parse_ndjson_stream()`: use `tokio::select! { biased; }` with `token.cancelled()` branch; on cancellation, call `finalize_blocks()`, emit `Error` with `StopReason::Aborted` and `"operation cancelled"` message

**Checkpoint**: All edge cases handled — adapter is robust against cancellation, stream interruptions, and message format variations

---

## Phase 12: Live Integration Tests

**Purpose**: End-to-end tests against a real Ollama instance (skipped by default via `#[ignore]`)

- [ ] T064 [P] Write live test `live_text_stream` in `adapters/tests/ollama_live.rs`: send simple prompt, verify Start/TextStart/TextDelta/TextEnd/Done events and non-empty text
- [ ] T065 [P] Write live test `live_usage_captured` in `adapters/tests/ollama_live.rs`: verify `Done` event has non-zero input and output tokens, and zero cost
- [ ] T066 [P] Write live test `live_tool_use_stream` in `adapters/tests/ollama_live.rs`: send prompt with a dummy tool definition, verify ToolCallStart with correct name and `StopReason::ToolUse`
- [ ] T067 [P] Write live test `live_multi_turn_context` in `adapters/tests/ollama_live.rs`: two-turn conversation where second turn references first; verify model recalls context
- [ ] T068 [P] Write live test `live_stop_reason_mapping` in `adapters/tests/ollama_live.rs`: simple prompt, verify `StopReason::Stop`
- [ ] T069 [P] Write live test `live_model_not_found` in `adapters/tests/ollama_live.rs`: use nonexistent model name, verify error event with HTTP error details

**Checkpoint**: All live tests pass against real Ollama instance

---

## Phase 13: Final Verification

**Purpose**: Full workspace build, test, and lint pass

- [ ] T070 Run `cargo build -p swink-agent-adapters` — verify clean compilation with no warnings
- [ ] T071 Run `cargo test -p swink-agent-adapters` — verify all unit tests pass (including Ollama-specific tests)
- [ ] T072 Run `cargo clippy --workspace -- -D warnings` — verify zero clippy warnings
- [ ] T073 Run `cargo test --workspace` — verify no regressions across the workspace
- [ ] T074 Verify `OllamaStreamFn` is accessible via `swink_agent_adapters::OllamaStreamFn` from external crate

**Checkpoint**: Feature complete — `OllamaStreamFn` is production-ready, all tests pass, zero warnings
