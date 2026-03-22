# Tasks: Adapter: Anthropic

**Input**: Design documents from `/specs/012-adapter-anthropic/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md
**Depends on**: 011-adapter-shared-infra (shared `AdapterBase`, `extract_tool_schemas`, `StreamFinalize`, SSE utilities)

**Tests**: Included — unit tests in `adapters/src/anthropic.rs` (`#[cfg(test)]` module) and live integration tests in `adapters/tests/anthropic_live.rs` (`#[ignore]`).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3, US4)
- Include exact file paths in descriptions

---

## Phase 1: Setup & Scaffolding

**Purpose**: Verify project structure, dependencies, and module wiring before implementing the adapter

- [x] T001 Verify `adapters/Cargo.toml` includes all required dependencies: `reqwest`, `futures`, `bytes`, `serde`, `serde_json`, `tokio`, `tokio-util`, `tracing`, and path dep on `swink-agent`
- [x] T002 Add `mod anthropic;` declaration to `adapters/src/lib.rs`
- [x] T003 Add `pub use anthropic::AnthropicStreamFn;` re-export to `adapters/src/lib.rs`
- [x] T004 Create `adapters/src/anthropic.rs` with `#![allow(...)]` annotations and module-level doc comment

---

## Phase 2: Request Types (Blocking Prerequisites)

**Purpose**: Define the serializable request types used to construct the Anthropic API request body. All user stories depend on these types.

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T005 Define `AnthropicContentBlock` enum in `adapters/src/anthropic.rs` with `Text`, `ToolUse`, `ToolResult` variants, each with `#[serde(tag = "type")]` serialization
- [x] T006 Define `AnthropicMessage` struct in `adapters/src/anthropic.rs` with `role: String` and `content: Vec<AnthropicContentBlock>`
- [x] T007 Define `AnthropicToolDef` struct in `adapters/src/anthropic.rs` with `name`, `description`, `input_schema: Value`
- [x] T008 Define `AnthropicThinking` struct in `adapters/src/anthropic.rs` with `type: String` (always `"enabled"`) and `budget_tokens: u64`
- [x] T009 Define `AnthropicChatRequest` struct in `adapters/src/anthropic.rs` with all fields per data model: `model`, `max_tokens`, `stream`, `system` (skip if None), `messages`, `tools` (skip if empty), `temperature` (skip if None), `thinking` (skip if None)

**Checkpoint**: Request types ready — all serializable types compile and can be serialized to JSON matching the Anthropic API format

---

## Phase 3: SSE Stream Types & State Machine

**Purpose**: Define the SSE parsing types and stream state tracking needed for all streaming user stories

- [x] T010 Define `BlockType` enum in `adapters/src/anthropic.rs` with `Text`, `Thinking`, `ToolUse` variants
- [x] T011 Define `SseStreamState` struct in `adapters/src/anthropic.rs` with `content_index: usize`, `active_blocks: HashMap<usize, (BlockType, usize)>`, `usage: Usage`, `stop_reason: Option<StopReason>`
- [x] T012 Define `SseLine` enum in `adapters/src/anthropic.rs` with `Event { event_type: String, data: String }` variant
- [x] T013 Implement `StreamFinalize` for `SseStreamState` via `drain_open_blocks()` that sorts active block indices, removes each, and maps to `OpenBlock` variants (Text, Thinking with None signature, ToolCall)

**Checkpoint**: Stream types ready — state machine types compile and `StreamFinalize` can drain open blocks

---

## Phase 4: Core Struct & StreamFn Trait

**Purpose**: Define the public `AnthropicStreamFn` struct and implement `StreamFn`

- [x] T014 Define `AnthropicStreamFn` struct in `adapters/src/anthropic.rs` with `base: AdapterBase` field
- [x] T015 Implement `AnthropicStreamFn::new(base_url, api_key)` constructor accepting `impl Into<String>` for both params, with `#[must_use]`
- [x] T016 Implement `Debug` for `AnthropicStreamFn` that redacts `api_key` as `"[REDACTED]"` while showing `base_url`
- [x] T017 Implement `StreamFn` for `AnthropicStreamFn` with `stream()` method delegating to `anthropic_stream()` helper
- [x] T018 Add compile-time `Send + Sync` assertion for `AnthropicStreamFn` via `const` block

**Checkpoint**: Public API ready — `AnthropicStreamFn` compiles, is `Send + Sync`, and implements `StreamFn`

---

## Phase 5: User Story 1 — Stream Text Responses from Anthropic (Priority: P1) MVP

**Goal**: Stream text responses incrementally from the Anthropic Messages API via SSE, emitting text deltas as they arrive.

**Independent Test**: Send a simple prompt and verify text deltas arrive incrementally and the final assembled message is coherent.

### Tests for User Story 1

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T019 [P] [US1] Write unit test verifying `convert_messages` produces correct `(system, messages)` for a simple user message with system prompt in `adapters/src/anthropic.rs` (`#[cfg(test)]` module)
- [x] T020 [P] [US1] Write unit test verifying `sse_event_lines` correctly pairs `event:` and `data:` lines into `SseLine::Event` values from a byte stream
- [x] T021 [P] [US1] Write unit test verifying `process_sse_event` emits `TextStart`, `TextDelta`, `TextEnd` for a text content block sequence
- [x] T022 [P] [US1] Write unit test verifying `process_sse_event` emits `Start` on first call and `Done` on `message_stop` with correct `Usage`

### Implementation for User Story 1

- [x] T023 [US1] Implement `convert_messages()` function that extracts system prompt as a separate `Option<String>`, converts `User` messages to text blocks, converts `Assistant` messages to text/tool-use blocks (filtering thinking and empty text), and merges consecutive `ToolResult` messages into single user messages
- [x] T024 [US1] Implement `send_request()` async function that constructs `AnthropicChatRequest`, sets `x-api-key` and `anthropic-version` headers, POSTs to `/v1/messages`, and returns `reqwest::Response` or error event
- [x] T025 [US1] Implement `sse_event_lines()` function that converts a byte stream into a stream of `SseLine::Event` values by pairing `event:` and `data:` lines, handling `\r\n` and `\n` line endings
- [x] T026 [US1] Implement `parse_sse_stream()` function using `stream::unfold` that emits `Start` on first call, then delegates to `process_sse_event()` for each SSE event, with cancellation support via `tokio::select!`
- [x] T027 [US1] Implement `process_sse_event()` handling for `message_start` (extract input/cache usage), `content_block_start` with `type: "text"` (emit `TextStart`), `content_block_delta` with `type: "text_delta"` (emit `TextDelta`), `content_block_stop` for text blocks (emit `TextEnd`)
- [x] T028 [US1] Implement `process_sse_event()` handling for `message_delta` (extract stop reason and output usage) and `message_stop` (finalize blocks, compute total usage, emit `Done`)
- [x] T029 [US1] Implement `anthropic_stream()` function that orchestrates: send request → check HTTP status → parse SSE stream, returning a flattened stream of events

**Checkpoint**: User Story 1 complete — text responses stream incrementally from the Anthropic API with correct event ordering and usage tracking

---

## Phase 6: User Story 2 — Stream Tool Call Responses from Anthropic (Priority: P1)

**Goal**: Stream tool call blocks with name, incremental JSON argument deltas, and completion events for agentic workflows.

**Independent Test**: Send a prompt with tool definitions and verify tool call events arrive with correct names and parseable arguments.

### Tests for User Story 2

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T030 [P] [US2] Write unit test verifying `process_sse_event` emits `ToolCallStart` with `id` and `name` for `content_block_start` with `type: "tool_use"`
- [x] T031 [P] [US2] Write unit test verifying `process_sse_event` emits `ToolCallDelta` for `content_block_delta` with `type: "input_json_delta"` containing `partial_json`
- [x] T032 [P] [US2] Write unit test verifying `process_sse_event` emits `ToolCallEnd` on `content_block_stop` for a tool-use block and removes it from `active_blocks`
- [x] T033 [P] [US2] Write unit test verifying `convert_messages` correctly converts `ToolCall` content blocks in assistant messages to `AnthropicContentBlock::ToolUse` and `ToolResult` messages to `AnthropicContentBlock::ToolResult`

### Implementation for User Story 2

- [x] T034 [US2] Implement `process_sse_event()` handling for `content_block_start` with `type: "tool_use"` — extract `id` and `name` from `/content_block/`, allocate content index, register in `active_blocks`, emit `ToolCallStart`
- [x] T035 [US2] Implement `process_sse_event()` handling for `content_block_delta` with `type: "input_json_delta"` — look up content index from `active_blocks`, extract `/delta/partial_json`, emit `ToolCallDelta`
- [x] T036 [US2] Implement `process_sse_event()` handling for `content_block_stop` for tool-use blocks — remove from `active_blocks`, emit `ToolCallEnd`
- [x] T037 [US2] Implement `extract_tool_schemas()` integration in `send_request()` — convert tool schemas to `AnthropicToolDef` format with `name`, `description`, `input_schema`

**Checkpoint**: User Story 2 complete — tool calls stream with correct names, incremental arguments, and completion events

---

## Phase 7: User Story 3 — Use Thinking Blocks with Budget Control (Priority: P2)

**Goal**: Support extended thinking with configurable token budgets, streaming thinking blocks as distinct events separate from text content.

**Independent Test**: Enable thinking with a budget, send a complex prompt, and verify thinking content arrives as distinct blocks with budget configuration in the request.

### Tests for User Story 3

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T038 [P] [US3] Write unit test verifying `resolve_thinking()` returns `None` when `ThinkingLevel::Off`
- [x] T039 [P] [US3] Write unit test verifying `resolve_thinking()` returns correct default budgets for each `ThinkingLevel` (Minimal=1024, Low=2048, Medium=5000, High=10000, ExtraHigh=20000)
- [x] T040 [P] [US3] Write unit test verifying `resolve_thinking()` caps budget to `max_tokens - 1` when budget >= max_tokens
- [x] T041 [P] [US3] Write unit test verifying `resolve_thinking()` uses custom budget from `ModelSpec.thinking_budgets` map when present
- [x] T042 [P] [US3] Write unit test verifying temperature is forced to `None` when thinking is enabled in `send_request()`
- [x] T043 [P] [US3] Write unit test verifying `process_sse_event` emits `ThinkingStart`, `ThinkingDelta`, `ThinkingEnd` for thinking content blocks

### Implementation for User Story 3

- [x] T044 [US3] Implement `resolve_thinking()` function that reads `ThinkingLevel` from `ModelSpec`, resolves budget from `thinking_budgets` map with hardcoded defaults as fallback, caps to `max_tokens - 1`, and returns `Option<AnthropicThinking>`
- [x] T045 [US3] Integrate `resolve_thinking()` into `send_request()` — call it, include result in `AnthropicChatRequest.thinking`, force temperature to `None` when thinking is enabled
- [x] T046 [US3] Implement `process_sse_event()` handling for `content_block_start` with `type: "thinking"` — allocate content index, register in `active_blocks` as `BlockType::Thinking`, emit `ThinkingStart`
- [x] T047 [US3] Implement `process_sse_event()` handling for `content_block_delta` with `type: "thinking_delta"` — look up content index, extract `/delta/thinking`, emit `ThinkingDelta`
- [x] T048 [US3] Implement `process_sse_event()` handling for `content_block_stop` for thinking blocks — remove from `active_blocks`, extract optional `/signature`, emit `ThinkingEnd` with signature

**Checkpoint**: User Story 3 complete — thinking blocks stream as distinct events with configurable budgets and temperature suppression

---

## Phase 8: User Story 4 — Handle Errors from Anthropic (Priority: P2)

**Goal**: Classify HTTP errors and SSE error events so the agent loop can apply appropriate retry strategies.

**Independent Test**: Simulate error responses (429, 401, 500, 529, network timeout) and verify each maps to the correct error type.

### Tests for User Story 4

> **NOTE: Write these tests FIRST, ensure they FAIL before implementation**

- [x] T049 [P] [US4] Write unit test verifying HTTP 401 maps to `error_auth()` (not retryable)
- [x] T050 [P] [US4] Write unit test verifying HTTP 429 maps to `error_throttled()` (retryable)
- [x] T051 [P] [US4] Write unit test verifying HTTP 529 maps to `error_network()` (retryable, Anthropic overloaded)
- [x] T052 [P] [US4] Write unit test verifying HTTP 504 maps to `error_network()` (retryable, gateway timeout)
- [x] T053 [P] [US4] Write unit test verifying HTTP 400-499 (other) maps to generic `error()` (not retryable)
- [x] T054 [P] [US4] Write unit test verifying HTTP 500-599 (other) maps to `error_network()` (retryable)
- [x] T055 [P] [US4] Write unit test verifying connection failure (reqwest error) maps to `error_network()`
- [x] T056 [P] [US4] Write unit test verifying SSE `error` event type extracts error message from `/error/message` JSON path and emits error event

### Implementation for User Story 4

- [x] T057 [US4] Implement HTTP error classification in `anthropic_stream()` — inline match on status code mapping to `AssistantMessageEvent` error constructors per the error classification contract
- [x] T058 [US4] Implement connection error handling in `send_request()` — map `reqwest::Error` to `error_network()` event
- [x] T059 [US4] Implement SSE `error` event handling in `process_sse_event()` — finalize open blocks, extract error message from JSON, emit error event

**Checkpoint**: User Story 4 complete — all Anthropic error conditions map to correct agent error types for retry strategy decisions

---

## Phase 9: Edge Cases & Finalization

**Purpose**: Handle edge cases identified in the spec and ensure clean stream finalization

### Tests for Edge Cases

- [x] T060 [P] Write unit test verifying unrecognized content block types in `content_block_start` are silently skipped (no panic, no error event)
- [x] T061 [P] Write unit test verifying `StreamFinalize::drain_open_blocks()` correctly drains all open blocks in sorted index order
- [x] T062 [P] Write unit test verifying cancellation emits finalization events for open blocks followed by an `Error` event with `StopReason::Aborted`
- [x] T063 [P] Write unit test verifying unexpected stream end (no `message_stop`) emits finalization events followed by an error event
- [x] T064 [P] Write unit test verifying `convert_messages` skips `CustomMessage` variants in the agent message log
- [x] T065 [P] Write unit test verifying consecutive `ToolResult` messages are merged into a single user message with multiple `tool_result` content blocks
- [x] T066 [P] Write unit test verifying empty text blocks in assistant messages are stripped during conversion
- [x] T067 [P] Write unit test verifying `message_start` correctly extracts `cache_read_input_tokens` and `cache_creation_input_tokens` into `Usage`

### Implementation for Edge Cases

- [x] T068 Implement `content_block_start` wildcard arm (`_ => {}`) for unrecognized block types
- [x] T069 Implement unexpected stream end handling in `parse_sse_stream()` — when `lines.next()` returns `None`, finalize open blocks and emit error event

**Checkpoint**: All edge cases handled — adapter is robust against unrecognized content, stream interruptions, and message format variations

---

## Phase 10: Integration & Verification

**Purpose**: Verify the complete adapter works end-to-end and passes all workspace checks

- [x] T070 Run `cargo build -p swink-agent-adapters` — verify clean compilation with no warnings
- [x] T071 Run `cargo test -p swink-agent-adapters` — verify all unit tests pass
- [x] T072 Run `cargo clippy --workspace -- -D warnings` — verify zero clippy warnings
- [x] T073 Run `cargo test --workspace` — verify no regressions across the workspace

**Checkpoint**: Feature complete — `AnthropicStreamFn` is production-ready, all tests pass, zero warnings
