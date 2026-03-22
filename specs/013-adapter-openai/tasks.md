# Tasks: Adapter: OpenAI

**Input**: Design documents from `/specs/013-adapter-openai/`
**Prerequisites**: plan.md (required), spec.md (required for user stories), research.md, data-model.md, contracts/public-api.md
**Depends on**: 011-adapter-shared-infra (shared `AdapterBase`, `MessageConverter`, `StreamFinalize`, SSE utilities), shared `openai_compat` types

**Tests**: Included — wiremock-based unit tests in `adapters/tests/openai.rs` and live integration tests in `adapters/tests/openai_live.rs` (`#[ignore]`).

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3, US4)
- Include exact file paths in descriptions

---

## Phase 1: Setup & Scaffolding

**Purpose**: Verify project structure, dependencies, and module wiring before implementing the adapter

- [x] T001 Verify `adapters/Cargo.toml` includes all required dependencies: `reqwest`, `futures`, `bytes`, `serde`, `serde_json`, `tokio`, `tokio-util`, `tracing`, `uuid`, `wiremock` (dev), and path dep on `swink-agent`
- [x] T002 Verify `mod openai;` declaration exists in `adapters/src/lib.rs`
- [x] T003 Verify `pub use openai::OpenAiStreamFn;` re-export exists in `adapters/src/lib.rs`
- [x] T004 Create `adapters/src/openai.rs` with `#![allow(...)]` annotations and module-level doc comment describing OpenAI-compatible adapter

**Checkpoint**: Module wiring complete — `OpenAiStreamFn` is importable from `swink_agent_adapters`

---

## Phase 2: Shared Request/Response Types (openai_compat.rs)

**Purpose**: Define or verify the shared OpenAI-compatible types used by OpenAI, Azure, Mistral, and xAI adapters. All user stories depend on these types.

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T005 [P] Define `OaiMessage` struct in `adapters/src/openai_compat.rs` with `role: String`, `content: Option<String>`, `tool_calls: Option<Vec<OaiToolCallRequest>>`, `tool_call_id: Option<String>`, all with `#[serde(skip_serializing_if)]` annotations
- [x] T006 [P] Define `OaiToolCallRequest` struct in `adapters/src/openai_compat.rs` with `id`, `type` (as `r#type`), `function: OaiFunctionCallRequest`
- [x] T007 [P] Define `OaiFunctionCallRequest` struct in `adapters/src/openai_compat.rs` with `name: String`, `arguments: String`
- [x] T008 [P] Define `OaiTool` struct in `adapters/src/openai_compat.rs` with `type` (as `r#type`), `function: OaiToolDef`
- [x] T009 [P] Define `OaiToolDef` struct in `adapters/src/openai_compat.rs` with `name`, `description`, `parameters: Value`
- [x] T010 [P] Define `OaiStreamOptions` struct in `adapters/src/openai_compat.rs` with `include_usage: bool`
- [x] T011 Define `OaiChatRequest` struct in `adapters/src/openai_compat.rs` with fields: `model`, `messages: Vec<OaiMessage>`, `stream: bool`, `stream_options: OaiStreamOptions`, `temperature: Option<f64>`, `max_tokens: Option<u64>`, `tools: Vec<OaiTool>` (skip_if empty), `tool_choice: Option<String>` (skip_if None)

**Checkpoint**: Request types ready — all serializable types compile and produce JSON matching the OpenAI API format

---

## Phase 3: SSE Response Types & State Machine

**Purpose**: Define the SSE streaming chunk deserialization types and stream state tracking

- [x] T012 [P] Define `OaiChunk` struct in `adapters/src/openai_compat.rs` with `choices: Vec<OaiChoice>` (`#[serde(default)]`), `usage: Option<OaiUsage>` (`#[serde(default)]`)
- [x] T013 [P] Define `OaiChoice` struct in `adapters/src/openai_compat.rs` with `delta: OaiDelta` (`#[serde(default)]`), `finish_reason: Option<String>` (`#[serde(default)]`)
- [x] T014 [P] Define `OaiDelta` struct (with `Default` derive) in `adapters/src/openai_compat.rs` with `content: Option<String>` (`#[serde(default)]`), `tool_calls: Option<Vec<OaiToolCallDelta>>` (`#[serde(default)]`)
- [x] T015 [P] Define `OaiToolCallDelta` struct in `adapters/src/openai_compat.rs` with `index: usize`, `id: Option<String>` (`#[serde(default)]`), `function: Option<OaiFunctionDelta>` (`#[serde(default)]`)
- [x] T016 [P] Define `OaiFunctionDelta` struct in `adapters/src/openai_compat.rs` with `name: Option<String>` (`#[serde(default)]`), `arguments: Option<String>` (`#[serde(default)]`)
- [x] T017 [P] Define `OaiUsage` struct in `adapters/src/openai_compat.rs` with `prompt_tokens: u64` (`#[serde(default)]`), `completion_tokens: u64` (`#[serde(default)]`)
- [x] T018 Define `ToolCallState` struct in `adapters/src/openai_compat.rs` with `arguments: String`, `started: bool`, `content_index: usize`
- [x] T019 Define `SseStreamState` struct in `adapters/src/openai.rs` with `text_started: bool`, `content_index: usize`, `tool_calls: HashMap<usize, ToolCallState>`, `usage: Option<Usage>`, `stop_reason: Option<StopReason>`
- [x] T020 Implement `StreamFinalize` for `SseStreamState` via `drain_open_blocks()` that drains open text blocks first, then open tool calls sorted by index

**Checkpoint**: Stream types ready — state machine types compile and `StreamFinalize` can drain open blocks

---

## Phase 4: MessageConverter & Helpers

**Purpose**: Implement message conversion from agent types to OpenAI wire format and tool schema helpers

- [x] T021 Implement `MessageConverter for OaiConverter` in `adapters/src/openai_compat.rs`: `system_message()` returns system-role `OaiMessage`, `user_message()` extracts text content, `assistant_message()` maps `ContentBlock::Text` to content and `ContentBlock::ToolCall` to `tool_calls` array, `tool_result_message()` maps to tool-role with `tool_call_id`
- [x] T022 Implement `build_oai_tools()` in `adapters/src/openai_compat.rs` that converts `&[Arc<dyn AgentTool>]` to `(Vec<OaiTool>, Option<String>)` using `extract_tool_schemas()`; returns `tool_choice: Some("auto")` when tools are non-empty, `None` otherwise

**Checkpoint**: Message conversion ready — agent messages convert to OpenAI wire format correctly

---

## Phase 5: Core Struct & StreamFn Trait

**Purpose**: Define the public `OpenAiStreamFn` struct and implement `StreamFn`

- [x] T023 Define `OpenAiStreamFn` struct in `adapters/src/openai.rs` with `pub(crate) base: AdapterBase` field
- [x] T024 Implement `OpenAiStreamFn::new(base_url, api_key)` constructor accepting `impl Into<String>` for both params, with `#[must_use]`
- [x] T025 Implement `Debug` for `OpenAiStreamFn` that redacts `api_key` as `"[REDACTED]"` while showing `base_url`, using `finish_non_exhaustive()`
- [x] T026 Implement `StreamFn` for `OpenAiStreamFn` with `stream()` method delegating to `openai_stream()` helper
- [x] T027 Add compile-time `Send + Sync` assertion for `OpenAiStreamFn` via `const` block

**Checkpoint**: Public API ready — `OpenAiStreamFn` compiles, is `Send + Sync`, and implements `StreamFn`

---

## Phase 6: User Story 1 — Stream Text Responses (Priority: P1) MVP

**Goal**: Stream text responses incrementally from any OpenAI-compatible chat completions API via SSE, emitting text deltas as they arrive.

**Independent Test**: Send a simple prompt and verify text deltas arrive incrementally and the final assembled message is coherent.

### Tests for User Story 1

- [x] T028 [US1] Write wiremock test `openai_text_stream` in `adapters/tests/openai.rs`: mock POST `/v1/chat/completions` with SSE body containing text content deltas, finish_reason `"stop"`, usage, and `[DONE]`; verify Start, TextStart, TextDelta, TextEnd, Done events are emitted in order
- [x] T029 [US1] Write wiremock test `openai_usage_captured` in `adapters/tests/openai.rs`: verify `Done` event contains correct `usage.input` and `usage.output` from the SSE usage chunk
- [x] T030 [US1] Write wiremock test `openai_usage_in_separate_chunk` in `adapters/tests/openai.rs`: simulate OpenAI's pattern where `finish_reason` and `usage` arrive in separate chunks before `[DONE]`; verify both are captured correctly
- [x] T031 [US1] Write wiremock test `openai_done_without_finish_reason` in `adapters/tests/openai.rs`: SSE stream has `[DONE]` but no `finish_reason` in any choice; verify Done event with `StopReason::Stop` default
- [x] T032 [US1] Write wiremock test `openai_empty_content_delta_skipped` in `adapters/tests/openai.rs`: first delta has empty content string; verify it is skipped and only the non-empty delta produces a TextDelta event
- [x] T033 [US1] Write wiremock test `openai_content_filter_stop_reason` in `adapters/tests/openai.rs`: `finish_reason: "content_filter"` maps to `StopReason::Stop` via wildcard match

### Implementation for User Story 1

- [x] T034 [US1] Implement `send_request()` in `adapters/src/openai.rs`: construct URL as `{base_url}/v1/chat/completions`, convert messages via `convert_messages::<OaiConverter>()`, build tools via `build_oai_tools()`, POST with Bearer auth (using `options.api_key` if present, falling back to `base.api_key`), return `reqwest::Response` or `error_network` event
- [x] T035 [US1] Implement `openai_stream()` in `adapters/src/openai.rs`: call `send_request()`, check HTTP status (classify errors per US4), call `parse_sse_stream()` for success
- [x] T036 [US1] Implement `parse_sse_stream()` in `adapters/src/openai.rs`: use `sse_data_lines()` to get line stream, `stream::unfold` with `SseStreamState`, emit `Start` on first iteration
- [x] T037 [US1] Handle text content deltas in `parse_sse_stream()`: on non-empty `choice.delta.content`, emit `TextStart` (once, tracked by `text_started`), then `TextDelta` with the delta string
- [x] T038 [US1] Handle `[DONE]` sentinel in `parse_sse_stream()`: on `SseLine::Done`, call `finalize_blocks()`, emit `Done` with saved `stop_reason` (default `StopReason::Stop`), saved `usage` (default `Usage::default()`), and `Cost::default()`
- [x] T039 [US1] Handle finish_reason mapping in `parse_sse_stream()`: `"tool_calls"` → `StopReason::ToolUse`, `"length"` → `StopReason::Length`, all others (including `"stop"`, `"content_filter"`) → `StopReason::Stop`; save to `state.stop_reason`, call `finalize_blocks()` to close open blocks
- [x] T040 [US1] Capture usage in `parse_sse_stream()`: when `chunk.usage` is present, map `prompt_tokens` → `input`, `completion_tokens` → `output`, set `cache_read: 0`, `cache_write: 0`, compute `total`

**Checkpoint US1**: Text streaming works end-to-end — wiremock tests pass, text deltas arrive incrementally, usage is captured, finish reasons are mapped correctly

---

## Phase 7: User Story 2 — Stream Tool Call Responses (Priority: P1)

**Goal**: Stream tool call chunks from the chat completions endpoint, including tool name, tool call ID, and incrementally-arriving JSON arguments.

**Independent Test**: Send a prompt with tool definitions and verify tool call events arrive with correct names, IDs, and parseable arguments.

### Tests for User Story 2

- [x] T041 [US2] Write wiremock test `openai_tool_call_stream` in `adapters/tests/openai.rs`: mock SSE body with tool call deltas (index 0, id `tc_1`, name `bash`, arguments split across chunks), finish_reason `"tool_calls"`; verify ToolCallStart, ToolCallDelta, ToolCallEnd events, correct id/name, and `StopReason::ToolUse`
- [x] T042 [US2] Write wiremock test `openai_text_then_tool` in `adapters/tests/openai.rs`: SSE starts with text content then switches to tool calls; verify TextEnd is emitted before ToolCallStart (text block closed before tool calls begin)
- [x] T043 [US2] Write wiremock test `openai_multiple_tool_calls` in `adapters/tests/openai.rs`: two parallel tool calls (index 0 and 1), each with separate id/name/arguments; verify both ToolCallStart events, sequential content indices (0 and 1), both ToolCallEnd events, and `StopReason::ToolUse`

### Implementation for User Story 2

- [x] T044 [US2] Implement `process_tool_call_delta()` in `adapters/src/openai.rs`: on vacant entry (new tool call index), extract `id` from delta (or generate `tc_{uuid}` fallback), extract `name` from function delta, allocate `content_index`, emit `ToolCallStart`, append any initial arguments and emit `ToolCallDelta`
- [x] T045 [US2] Handle existing tool call deltas in `process_tool_call_delta()`: on occupied entry, append arguments to accumulated string, emit `ToolCallDelta` with the delta fragment
- [x] T046 [US2] Close text block before tool calls in `parse_sse_stream()`: when `choice.delta.tool_calls` is present and `text_started` is true, emit `TextEnd`, reset `text_started` to false, increment `content_index`

**Checkpoint US2**: Tool call streaming works — wiremock tests pass, parallel tool calls have separate content indices, text blocks are properly closed before tool calls

---

## Phase 8: User Story 3 — Alternative Provider Compatibility (Priority: P2)

**Goal**: Ensure the adapter works with non-OpenAI providers that implement the same SSE protocol with minor variations.

**Independent Test**: Configure with different base URLs and verify streaming works correctly despite missing/sparse fields.

### Tests for User Story 3

- [x] T047 [US3] Write wiremock test `openai_empty_choices_array` in `adapters/tests/openai.rs`: SSE chunks include empty `choices: []` arrays interspersed with valid chunks; verify the adapter skips them gracefully and completes normally
- [x] T048 [US3] Write wiremock test `openai_missing_done_sentinel` in `adapters/tests/openai.rs`: SSE stream ends (connection closes) without `[DONE]` or `finish_reason`; verify Error event with "stream ended unexpectedly" and open text block is finalized via `TextEnd`
- [x] T049 [US3] Write wiremock test `openai_empty_response_body` in `adapters/tests/openai.rs`: empty SSE body (200 OK but no data); verify Start is emitted then Error about unexpected stream end

### Implementation for User Story 3

- [x] T050 [US3] Handle stream end without `[DONE]` in `parse_sse_stream()`: on `None` from line stream, call `finalize_blocks()`, if `stop_reason` was saved emit `Done` (valid provider that omits `[DONE]`), otherwise emit `error("OpenAI stream ended unexpectedly")`
- [x] T051 [US3] Ensure all deserialization types use `#[serde(default)]` on optional and collection fields so sparse JSON from alternative providers parses without error

**Checkpoint US3**: Alternative provider compatibility verified — empty choices, missing `[DONE]`, sparse fields all handled gracefully

---

## Phase 9: User Story 4 — Error Handling (Priority: P2)

**Goal**: Classify HTTP errors from OpenAI-compatible providers for correct retry behavior.

**Independent Test**: Simulate error responses and verify each maps to the correct error type.

### Tests for User Story 4

- [x] T052 [US4] Write wiremock test `openai_http_401` in `adapters/tests/openai.rs`: mock 401 response; verify Error event contains "auth error"
- [x] T053 [US4] Write wiremock test `openai_http_429` in `adapters/tests/openai.rs`: mock 429 response; verify Error event contains "rate limit"
- [x] T054 [US4] Write wiremock test `openai_http_500` in `adapters/tests/openai.rs`: mock 500 response; verify Error event contains "server error"
- [x] T055 [US4] Write wiremock test `openai_malformed_json` in `adapters/tests/openai.rs`: SSE data line with invalid JSON; verify Error event contains "parse error" or "JSON"

### Implementation for User Story 4

- [x] T056 [US4] Implement HTTP error classification in `openai_stream()`: 401/403 → `error_auth()`, 429 → `error_throttled()`, 500–599 → `error_network()`, other 4xx → `error()` (generic)
- [x] T057 [US4] Handle connection errors in `send_request()`: `reqwest` send failure → `error_network()` event
- [x] T058 [US4] Handle JSON parse errors in `parse_sse_stream()`: `serde_json::from_str` failure → log error, call `finalize_blocks()`, emit `error()` event

**Checkpoint US4**: Error handling verified — all HTTP error codes and failure modes map to correct error constructors

---

## Phase 10: Cancellation & Auth

**Purpose**: Verify cancellation support and authentication behavior

### Tests

- [x] T059 Write wiremock test `openai_cancellation` in `adapters/tests/openai.rs`: start slow SSE stream, cancel via `CancellationToken` after 50ms; verify `Error` event with `StopReason::Aborted` and "operation cancelled" message
- [x] T060 Write wiremock test `openai_bearer_token_sent` in `adapters/tests/openai.rs`: mock expects `Authorization: Bearer test-key` header; verify request succeeds (Start event emitted)
- [x] T061 Write wiremock test `openai_stream_options_api_key_overrides_default` in `adapters/tests/openai.rs`: create `OpenAiStreamFn` with "default-key" but pass `StreamOptions { api_key: Some("override-key") }`; mock expects `Bearer override-key`; verify override is used
- [x] T062 Write wiremock test `openai_debug_redacts_key` in `adapters/tests/openai.rs`: create with secret key, format as `{:?}`, verify output contains `[REDACTED]` and does not contain the actual key

### Implementation

- [x] T063 Handle cancellation in `parse_sse_stream()`: use `tokio::select! { biased; }` with `token.cancelled()` branch; on cancellation, call `finalize_blocks()`, emit `Error` with `StopReason::Aborted`

**Checkpoint**: Cancellation and auth verified

---

## Phase 11: Live Integration Tests

**Purpose**: End-to-end tests against the real OpenAI API (skipped by default)

- [x] T064 [P] Write live test `live_text_stream` in `adapters/tests/openai_live.rs`: send simple prompt, verify Start/TextStart/TextDelta/TextEnd/Done events and non-empty text
- [x] T065 [P] Write live test `live_usage_and_cost` in `adapters/tests/openai_live.rs`: verify `Done` event has non-zero input and output tokens
- [x] T066 [P] Write live test `live_tool_use_stream` in `adapters/tests/openai_live.rs`: send prompt with `DummyTool` (get_weather), verify ToolCallStart with correct name and `StopReason::ToolUse`
- [x] T067 [P] Write live test `live_multi_turn_context` in `adapters/tests/openai_live.rs`: two-turn conversation where second turn references first; verify model recalls context
- [x] T068 [P] Write live test `live_stop_reason_mapping` in `adapters/tests/openai_live.rs`: simple prompt, verify `StopReason::Stop`
- [x] T069 [P] Write live test `live_invalid_key_returns_auth_error` in `adapters/tests/openai_live.rs`: use bogus key, verify Error event with auth-related message

**Checkpoint**: All live tests pass against real OpenAI API

---

## Phase 12: Final Verification

**Purpose**: Full workspace build, test, and lint pass

- [x] T070 Run `cargo test -p swink-agent-adapters` — all wiremock-based OpenAI tests pass
- [x] T071 Run `cargo test --workspace` — no regressions across workspace
- [x] T072 Run `cargo clippy --workspace -- -D warnings` — zero warnings
- [x] T073 Verify `OpenAiStreamFn` is accessible via `swink_agent_adapters::OpenAiStreamFn` from external crate

**Checkpoint**: Feature complete — adapter implemented, all tests pass, zero clippy warnings
