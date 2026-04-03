# Tasks: Adapter AWS Bedrock

**Input**: Design documents from `/specs/019-adapter-bedrock/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Dependencies & Workspace)

**Purpose**: Add new workspace dependencies and configure feature gates for event-stream parsing

- [x] T001 Add `aws-smithy-eventstream` (0.60.x) and `aws-smithy-types` (1.x) to workspace `[workspace.dependencies]` in Cargo.toml
- [x] T002 Add `aws-smithy-eventstream` and `aws-smithy-types` as optional deps gated behind `bedrock` feature in adapters/Cargo.toml (alongside existing `sha2`, `hmac`, `chrono`)
- [x] T003 Verify `cargo build -p swink-agent-adapters --no-default-features --features bedrock` compiles with new deps

**Checkpoint**: Bedrock feature compiles with event-stream parsing crates available

---

## Phase 2: Foundational (Request & Type Updates)

**Purpose**: Update request types and message conversion to support ConverseStream API — blocks all user stories

**CRITICAL**: No user story work can begin until this phase is complete

- [x] T004 Add `BedrockSystemBlock` struct (`text: String`) and add `system: Option<Vec<BedrockSystemBlock>>` field to `BedrockRequest` in adapters/src/bedrock.rs
- [x] T005 Update `build_request()` in adapters/src/bedrock.rs to populate the `system` field from `context.system_prompt` instead of prepending a synthetic user message
- [x] T006 Add streaming event deserialization types in adapters/src/bedrock.rs: `MessageStartEvent`, `ContentBlockStartEvent` (with `StartBlock` enum for text/toolUse), `ContentBlockDeltaEvent` (with `DeltaBlock` enum for text/toolUse), `ContentBlockStopEvent`, `MessageStopEvent`, `MetadataEvent` (with `BedrockStreamUsage` and `BedrockMetrics`)
- [x] T007 Add `BedrockStreamState` struct in adapters/src/bedrock.rs to track streaming state: `current_block_type: Option<BlockType>` (Text/ToolUse), `stop_reason: Option<String>`, `usage: Option<Usage>`, `content_index: usize`
- [x] T008 Add `parse_event_frame()` function in adapters/src/bedrock.rs that takes an `aws_smithy_eventstream` `Message`, reads the `:event-type` header, deserializes the JSON payload into the appropriate event type, and returns `Option<Vec<AssistantMessageEvent>>`
- [x] T009 Add `map_stop_reason()` helper in adapters/src/bedrock.rs: `end_turn`/`stop_sequence` → `Stop`, `tool_use` → `ToolUse`, `max_tokens` → `Length`, `guardrail_intervened` → emit `ContentFiltered` error event

**Checkpoint**: All streaming types defined, event parsing function compiles, `cargo test -p swink-agent-adapters --features bedrock` passes

---

## Phase 3: User Story 1 — Stream Text Responses from AWS Bedrock (Priority: P1) MVP

**Goal**: Stream text content incrementally from the Bedrock ConverseStream endpoint

**Independent Test**: Send a simple prompt to Bedrock and verify text deltas arrive incrementally with a terminal Done event

### Implementation for User Story 1

- [ ] T010 [US1] Replace the `converse()` method with `converse_stream()` in adapters/src/bedrock.rs: change URL path from `/model/{id}/converse` to `/model/{id}/converse-stream`, read response as `bytes_stream()`, feed into `MessageFrameDecoder`, yield `AssistantMessageEvent`s via `stream::unfold` with `BedrockStreamState`
- [ ] T011 [US1] Update the `StreamFn::stream()` impl in adapters/src/bedrock.rs to call `converse_stream()` instead of `converse()`, integrating `tokio::select!` for cancellation_token support in the streaming loop
- [ ] T012 [US1] Wire up `messageStart` → `Start`, `contentBlockStart(text)` → `TextStart`, `contentBlockDelta(text)` → `TextDelta`, `contentBlockStop` → `TextEnd`, `metadata` → `Done` event mapping in `parse_event_frame()` for text blocks
- [ ] T013 [US1] Handle `messageStop` event in `parse_event_frame()`: capture `stopReason` into `BedrockStreamState`, emit `Done` event with usage from subsequent `metadata` event
- [ ] T014 [US1] Add unit test `text_event_stream_parsing` in adapters/src/bedrock.rs that constructs mock event-stream frames for a text response and verifies correct `AssistantMessageEvent` sequence (Start, TextStart, TextDelta, TextEnd, Done)
- [ ] T015 [US1] Create adapters/tests/bedrock_live.rs with module-level cfg gate, imports, constants (TIMEOUT = 30s), and helper functions: `aws_creds()` (reads AWS env vars via dotenvy), `cheap_model()` (cheapest available model), `simple_context()`, `collect_events()`, `event_name()`
- [ ] T016 [US1] Add `live_text_stream` test in adapters/tests/bedrock_live.rs: send simple prompt, assert Start/TextStart/TextDelta/TextEnd/Done events present and assembled text is non-empty
- [ ] T017 [US1] Add `live_usage_and_cost` test in adapters/tests/bedrock_live.rs: send simple prompt, assert Done event contains non-zero input and output token counts

**Checkpoint**: `cargo test -p swink-agent-adapters --features bedrock` passes; live text streaming tests pass with AWS credentials

---

## Phase 4: User Story 2 — Stream Tool Call Responses from AWS Bedrock (Priority: P1)

**Goal**: Stream tool calls with names, IDs, and incrementally-arriving JSON arguments

**Independent Test**: Send a prompt with tool definitions and verify ToolCallStart/ToolCallDelta/ToolCallEnd events with correct tool name and valid JSON args

### Implementation for User Story 2

- [ ] T018 [US2] Wire up `contentBlockStart(toolUse)` → `ToolCallStart` (with `toolUseId`, `name`), `contentBlockDelta(toolUse)` → `ToolCallDelta` (with `input` partial JSON), `contentBlockStop` → `ToolCallEnd` event mapping in `parse_event_frame()`
- [ ] T019 [US2] Add unit test `tool_call_event_stream_parsing` in adapters/src/bedrock.rs that constructs mock event-stream frames for a tool call response and verifies correct ToolCallStart/ToolCallDelta/ToolCallEnd sequence with valid accumulated JSON
- [ ] T020 [US2] Add `DummyTool` struct (get_weather) in adapters/tests/bedrock_live.rs implementing `AgentTool` with JSON schema for city parameter
- [ ] T021 [US2] Add `live_tool_use_stream` test in adapters/tests/bedrock_live.rs: send prompt with get_weather tool, assert ToolCallStart with name "get_weather", ToolCallEnd present, and StopReason::ToolUse
- [ ] T022 [US2] Add `live_multi_turn_context` test in adapters/tests/bedrock_live.rs: send two-turn conversation (introduce name, then ask for recall), assert second reply contains the introduced name

**Checkpoint**: Tool call streaming works; live tests pass for both text and tool call scenarios

---

## Phase 5: User Story 3 — Authenticate with AWS SigV4 Request Signing (Priority: P2)

**Goal**: Verify SigV4 signing works correctly with the ConverseStream endpoint (signing logic already exists in stub)

**Independent Test**: Verify correct SigV4 headers are generated and invalid credentials produce auth errors

### Implementation for User Story 3

- [ ] T023 [US3] Verify that the existing SigV4 signing in `converse_stream()` correctly signs the `/converse-stream` URL path (update path in signing if it was hardcoded to `/converse`)
- [ ] T024 [US3] Add unit test `sigv4_signs_converse_stream_path` in adapters/src/bedrock.rs that verifies the canonical request uses the `/model/{id}/converse-stream` path
- [ ] T025 [US3] Add `live_invalid_creds_returns_auth_error` test in adapters/tests/bedrock_live.rs: create BedrockStreamFn with bogus credentials, assert Error event with auth-related message

**Checkpoint**: SigV4 signing verified for streaming endpoint; auth error test passes

---

## Phase 6: User Story 4 — Handle Errors from AWS Bedrock (Priority: P2)

**Goal**: Verify HTTP error codes are classified correctly via the shared error classifier

**Independent Test**: Trigger auth error with invalid credentials and verify correct error classification

### Implementation for User Story 4

- [ ] T026 [US4] Handle event-stream exception frames (`:message-type` = `"exception"`) in the streaming loop in adapters/src/bedrock.rs: extract `:exception-type` header and payload, emit appropriate error event
- [ ] T027 [US4] Add `guardrail_intervened` handling in `map_stop_reason()` in adapters/src/bedrock.rs: when stopReason is `guardrail_intervened`, emit `ContentFiltered` error event instead of normal Done
- [ ] T028 [US4] Add unit test `exception_frame_handling` in adapters/src/bedrock.rs that constructs a mock exception frame (`:message-type` = `"exception"`, `:exception-type` = `"throttlingException"`) and verifies it maps to a throttled error event
- [ ] T029 [US4] Add unit test `guardrail_intervened_maps_to_content_filtered` in adapters/src/bedrock.rs that verifies `guardrail_intervened` stop reason produces a ContentFiltered error event

**Checkpoint**: All error paths covered; exception frames and guardrail blocks handled correctly

---

## Phase 7: Model Catalog Update

**Purpose**: Expand Bedrock model presets from 5 to ~50 models across all provider families

- [ ] T030 [P] Update Bedrock provider presets in src/model_catalog.toml: add Anthropic models (Opus 4.6, Sonnet 4.6, Haiku 4.5, 3.7 Sonnet, 3.5 Sonnet v2, 3.5 Haiku, 3 Opus, 3 Haiku) — update existing Sonnet 4.5 entry if needed
- [ ] T031 [P] Add Meta Llama models to Bedrock presets in src/model_catalog.toml: Llama 4 Scout, 3.3 70B, 3.2 (90B/11B/3B/1B), 3.1 (405B/70B/8B) — update existing Maverick entry
- [ ] T032 [P] Add Amazon Nova models to Bedrock presets in src/model_catalog.toml: Nova 2 Pro, 2 Lite, Lite v1, Micro v1, Premier v1 — update existing Nova Pro entry
- [ ] T033 [P] Add Mistral models to Bedrock presets in src/model_catalog.toml: Large 3 (2512), Large (2407), Ministral 3 (14B/8B/3B), Small, Mixtral 8x7B, 7B — update existing Pixtral Large entry
- [ ] T034 [P] Add DeepSeek, AI21, Cohere, OpenAI, Qwen, Writer, and other provider models to Bedrock presets in src/model_catalog.toml
- [ ] T035 Update `remote_presets.rs` preset key constants in `pub mod bedrock` to include all new model presets, and update the `added_provider_presets_map_to_catalog_models` test

**Checkpoint**: `cargo test -p swink-agent-adapters --features bedrock` passes with expanded catalog; all preset keys map to valid catalog entries

---

## Phase 8: Polish & Verification

**Purpose**: Build verification, clippy clean, feature-gate isolation, documentation

- [ ] T036 Run `cargo build --workspace` and verify clean compilation
- [ ] T037 Run `cargo test --workspace` and verify all tests pass
- [ ] T038 Run `cargo clippy --workspace -- -D warnings` and verify zero warnings
- [ ] T039 Run `cargo test -p swink-agent-adapters --no-default-features --features bedrock` and verify bedrock feature compiles and runs in isolation
- [ ] T040 Update adapters/CLAUDE.md to change bedrock status from "Stub" to "Implemented" in the feature gates table, update extra deps to include `aws-smithy-eventstream`, `aws-smithy-types`
- [ ] T041 Remove old non-streaming `BedrockResponse`, `BedrockOutput`, `BedrockOutputMessage`, `BedrockOutputContentBlock` types from adapters/src/bedrock.rs (replaced by streaming event types)

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Setup)**: No dependencies — start immediately
- **Phase 2 (Foundational)**: Depends on Phase 1 (needs new deps available)
- **Phase 3 (US1 Text Streaming)**: Depends on Phase 2 (needs streaming types + event parser)
- **Phase 4 (US2 Tool Calls)**: Depends on Phase 3 (extends event parser, reuses test infrastructure)
- **Phase 5 (US3 SigV4 Auth)**: Depends on Phase 3 (needs streaming path to verify signing)
- **Phase 6 (US4 Error Handling)**: Depends on Phase 3 (needs streaming loop for exception frames)
- **Phase 7 (Model Catalog)**: Independent of Phases 3-6 (data-only changes to TOML)
- **Phase 8 (Polish)**: Depends on all previous phases

### User Story Dependencies

- **US1 (Text Streaming)**: Foundation for all other stories
- **US2 (Tool Calls)**: Extends US1 event parser with tool call mapping
- **US3 (SigV4 Auth)**: Independent verification of existing signing with new endpoint
- **US4 (Error Handling)**: Extends US1 streaming loop with exception frame handling

### Parallel Opportunities

- T004, T005, T006, T007 are independent struct/type additions (but same file)
- T030, T031, T032, T033, T034 are parallel (different TOML sections, independent model families)
- Phase 7 (Catalog) can run in parallel with Phases 4-6
- T036, T037, T038 are sequential (build → test → clippy)

---

## Parallel Example: Model Catalog

```bash
# All catalog tasks can run in parallel (different TOML sections):
Task T030: "Add Anthropic models to Bedrock presets in src/model_catalog.toml"
Task T031: "Add Meta Llama models to Bedrock presets in src/model_catalog.toml"
Task T032: "Add Amazon Nova models to Bedrock presets in src/model_catalog.toml"
Task T033: "Add Mistral models to Bedrock presets in src/model_catalog.toml"
Task T034: "Add DeepSeek/AI21/Cohere/OpenAI/Qwen/Writer models in src/model_catalog.toml"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Add deps
2. Complete Phase 2: Streaming types + event parser
3. Complete Phase 3: Text streaming with live tests
4. **STOP and VALIDATE**: Run live tests with AWS credentials
5. Adapter is usable for basic text generation

### Incremental Delivery

1. Phase 1 + 2 → Foundation ready (deps + types)
2. Phase 3 → Text streaming verified (MVP!)
3. Phase 4 → Tool calls verified
4. Phase 5 → SigV4 signing verified for streaming
5. Phase 6 → Error handling + guardrails
6. Phase 7 → Comprehensive model catalog
7. Phase 8 → Clean build, docs updated

---

## Notes

- The existing `bedrock.rs` stub has ~600 lines of reusable code (SigV4 signing, request types, message conversion, crypto helpers)
- The main rewrite is replacing `converse()` with streaming `converse_stream()` and adding event-stream frame parsing
- Two new workspace deps: `aws-smithy-eventstream` (frame decoder) and `aws-smithy-types` (event-stream types)
- Live tests require `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`, and Bedrock model access
- System prompt now uses native Bedrock `system` field instead of synthetic user message
