# Tasks: Adapter xAI

**Input**: Design documents from `/specs/017-adapter-xai/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Model Catalog & Presets)

**Purpose**: Update stale model catalog and remote preset constants to current Grok 4.x models

- [ ] T001 Replace stale grok-3 and grok-3-fast presets with 5 current grok-4.x model entries in src/model_catalog.toml (grok-4.20-0309-reasoning, grok-4.20-0309-non-reasoning, grok-4-1-fast-reasoning, grok-4-1-fast-non-reasoning, grok-4.20-multi-agent-0309; all 2M context, capabilities: text/tools/images_in/streaming/structured_output)
- [ ] T002 Update RemotePresetKey constants in adapters/src/remote_presets.rs: replace GROK_3/GROK_3_FAST with GROK_4_20_REASONING, GROK_4_20_NON_REASONING, GROK_4_1_FAST_REASONING, GROK_4_1_FAST_NON_REASONING, GROK_4_20_MULTI_AGENT
- [ ] T003 Update the added_provider_presets_map_to_catalog_models test in adapters/src/remote_presets.rs to use new preset keys and verify correct model_id mapping

**Checkpoint**: `cargo test -p swink-agent-adapters --features xai` passes with updated catalog and preset constants

---

## Phase 2: User Story 1 — Stream Text Responses from xAI (Priority: P1) MVP

**Goal**: Verify that text content streams incrementally from the xAI endpoint via SSE

**Independent Test**: Send a simple prompt to the xAI endpoint and verify text deltas arrive incrementally with a terminal Done event

### Implementation for User Story 1

- [ ] T004 [US1] Create adapters/tests/xai_live.rs with module-level cfg gate, imports, constants (TIMEOUT = 30s), and helper functions: xai_key() (reads XAI_API_KEY via dotenvy), cheap_model() (grok-4-1-fast-non-reasoning), simple_context(), collect_events(), event_name()
- [ ] T005 [US1] Add live_text_stream test in adapters/tests/xai_live.rs: send simple prompt, assert Start/TextStart/TextDelta/TextEnd/Done events present and assembled text is non-empty
- [ ] T006 [US1] Add live_usage_and_cost test in adapters/tests/xai_live.rs: send simple prompt, assert Done event contains non-zero input and output token counts
- [ ] T007 [US1] Add live_stop_reason_mapping test in adapters/tests/xai_live.rs: send simple prompt, assert Done event has StopReason::Stop

**Checkpoint**: `cargo test -p swink-agent-adapters --test xai_live -- --ignored` passes for text streaming tests (requires XAI_API_KEY)

---

## Phase 3: User Story 2 — Stream Tool Call Responses from xAI (Priority: P1)

**Goal**: Verify that tool calls stream correctly with names, IDs, and parseable JSON arguments

**Independent Test**: Send a prompt with tool definitions and verify ToolCallStart/ToolCallEnd events with correct tool name and valid JSON args

### Implementation for User Story 2

- [ ] T008 [US2] Add DummyTool struct (get_weather) in adapters/tests/xai_live.rs implementing AgentTool with JSON schema for city parameter
- [ ] T009 [US2] Add live_tool_use_stream test in adapters/tests/xai_live.rs: send prompt with get_weather tool, assert ToolCallStart with name "get_weather", ToolCallEnd present, and StopReason::ToolUse
- [ ] T010 [US2] Add live_multi_turn_context test in adapters/tests/xai_live.rs: send two-turn conversation (introduce name, then ask for recall), assert second reply contains the introduced name

**Checkpoint**: `cargo test -p swink-agent-adapters --test xai_live -- --ignored` passes for all tool call tests

---

## Phase 4: User Story 3 — Connect to xAI-Specific Endpoint (Priority: P2)

**Goal**: Verify that the adapter targets the correct xAI endpoint with proper Bearer authentication

**Independent Test**: Verify correct URL construction and auth header by testing with invalid credentials

### Implementation for User Story 3

- [ ] T011 [US3] Add live_invalid_key_returns_auth_error test in adapters/tests/xai_live.rs: create XAiStreamFn with bogus key, assert Error event with auth-related message

**Checkpoint**: Auth error test passes confirming correct endpoint targeting and error classification

---

## Phase 5: User Story 4 — Handle Errors from xAI (Priority: P2)

**Goal**: Verify HTTP error codes are classified correctly via the shared error classifier

**Independent Test**: Trigger auth error with invalid key and verify correct error classification

### Implementation for User Story 4

- [ ] T012 [US4] Verify error classification is handled by existing shared infra in adapters/src/classify.rs — no xAI-specific error handling code needed (covered by T011 auth error test and shared classifier unit tests)

**Checkpoint**: All error classification verified through existing shared tests + live auth error test

---

## Phase 6: Polish & Verification

**Purpose**: Build verification, clippy clean, feature-gate isolation

- [ ] T013 Run cargo build --workspace and verify clean compilation
- [ ] T014 Run cargo test --workspace and verify all tests pass
- [ ] T015 Run cargo clippy --workspace -- -D warnings and verify zero warnings
- [ ] T016 Run cargo test -p swink-agent-adapters --no-default-features --features xai and verify xai feature compiles and runs in isolation
- [ ] T017 Update adapters/CLAUDE.md to change xai status from "Stub" to "Implemented" in the feature gates table

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Setup)**: No dependencies — start immediately
- **Phase 2 (US1 Text Streaming)**: Depends on Phase 1 (needs correct model IDs for cheap_model())
- **Phase 3 (US2 Tool Calls)**: Depends on Phase 2 (reuses test helpers from T004)
- **Phase 4 (US3 Endpoint)**: Depends on Phase 2 (reuses test helpers from T004)
- **Phase 5 (US4 Errors)**: Depends on Phase 4 (T011 covers the error case)
- **Phase 6 (Polish)**: Depends on all previous phases

### User Story Dependencies

- **US1 (Text Streaming)**: Independent after Phase 1
- **US2 (Tool Calls)**: Shares test file with US1 but independent test scenarios
- **US3 (Endpoint)**: Independent test (invalid key)
- **US4 (Error Handling)**: Covered by shared infra + US3 auth error test

### Parallel Opportunities

- T001, T002, T003 can run sequentially (same files/dependencies)
- T005, T006, T007 are parallel (independent test functions, same file)
- T009, T010 are parallel (independent test functions)
- T013, T014, T015 are sequential (build → test → clippy)

---

## Parallel Example: User Story 1

```bash
# After T004 creates test infrastructure, launch US1 tests in parallel:
Task T005: "live_text_stream test in adapters/tests/xai_live.rs"
Task T006: "live_usage_and_cost test in adapters/tests/xai_live.rs"
Task T007: "live_stop_reason_mapping test in adapters/tests/xai_live.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Update model catalog + preset constants
2. Complete Phase 2: Text streaming live tests
3. **STOP and VALIDATE**: Run live tests with XAI_API_KEY
4. Adapter is usable for basic text generation

### Incremental Delivery

1. Phase 1 → Catalog current, presets wired
2. Phase 2 → Text streaming verified (MVP!)
3. Phase 3 → Tool calls verified
4. Phase 4 + 5 → Error handling verified
5. Phase 6 → Clean build, docs updated

---

## Notes

- The adapter code (adapters/src/xai.rs) already exists and is complete — no code changes needed there
- Feature gate, lib.rs re-export, and Cargo.toml entry are already in place
- All live tests require XAI_API_KEY and use grok-4-1-fast-non-reasoning (cheapest model at $0.20/$0.50 per 1M tokens)
- Tests follow the established pattern from openai_live.rs
