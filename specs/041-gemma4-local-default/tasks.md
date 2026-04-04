# Tasks: Gemma 4 Local Default (Direct Inference)

**Input**: Design documents from `/specs/041-gemma4-local-default/`
**Prerequisites**: plan.md (required), spec.md (required), research.md, data-model.md

**Tests**: Included per constitution (Test-Driven Development is NON-NEGOTIABLE). Tests are written before implementation.

**Organization**: Tasks are grouped by user story to enable independent implementation and testing of each story.

**NaN Risk**: mistral.rs #2051 (NaN logits) was reported against MoE variants (26B) with BF16/UQFF. E2B (dense, Q4_K_M GGUF) is likely unaffected. Implementation proceeds for E2B with a **live validation gate** after US1 (Phase 3). E4B/26B MoE presets deferred until upstream MoE fix ships. All tasks assume mistralrs 0.8.0.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Feature Flag & Dependency)

**Purpose**: Add `gemma4` feature flag and upgrade mistralrs dependency

- [x] T001 Add `gemma4` feature flag to `local-llm/Cargo.toml` with empty feature gate (no forwarded features initially)
- [x] T002 Bump `mistralrs` from `"0.7"` to `"0.8"` in `local-llm/Cargo.toml`
- [x] T003 Fix any compilation errors from mistralrs 0.7 → 0.8 API changes in `local-llm/src/model.rs`, `local-llm/src/stream.rs`, `local-llm/src/embedding.rs`
- [x] T004 Verify `cargo build -p swink-agent-local-llm` succeeds with existing SmolLM3-3B code on mistralrs 0.8
- [x] T005 Verify `cargo test -p swink-agent-local-llm` — all existing tests pass on mistralrs 0.8

**Checkpoint**: mistralrs 0.8 is integrated, all existing functionality works unchanged

---

## Phase 2: Foundational (Model-Family Detection)

**Purpose**: Core infrastructure that all Gemma 4 user stories depend on

**CRITICAL**: No Gemma 4 user story work can begin until this phase is complete

### Tests

- [x] T006 [P] Write test `is_gemma4_detects_bartowski_repo` — `ModelConfig` with `repo_id = "bartowski/google_gemma-4-E2B-it-GGUF"` returns `true` in `local-llm/src/model.rs`
- [x] T007 [P] Write test `is_gemma4_detects_ollama_style_repo` — `ModelConfig` with `repo_id = "gemma4-e2b"` returns `true` in `local-llm/src/model.rs`
- [x] T008 [P] Write test `is_gemma4_false_for_smollm` — `ModelConfig` with `repo_id = "bartowski/SmolLM3-3B-GGUF"` returns `false` in `local-llm/src/model.rs`

### Implementation

- [x] T009 Add `ModelConfig::is_gemma4(&self) -> bool` method behind `#[cfg(feature = "gemma4")]` in `local-llm/src/model.rs` — checks `repo_id` for `"gemma-4"` or `"gemma4"` substrings
- [x] T010 Verify T006-T008 tests pass with `--features gemma4`

**Checkpoint**: Foundation ready — model-family detection works, user story implementation can begin

---

## Phase 3: User Story 1 — Run Gemma 4 E2B Locally (Priority: P1) MVP

**Goal**: Developers can run Gemma 4 E2B/E4B/26B via direct inference without Ollama. Model downloads from HuggingFace on first use, streams responses.

**Independent Test**: Configure local-llm crate with Gemma 4 preset, send a prompt, verify streaming text output without external service dependency.

### Tests for User Story 1

- [x] T011 [P] [US1] Write test `gemma4_e2b_preset_config_defaults` — verify repo_id, filename, context_length (131072) for `ModelPreset::Gemma4_E2B` in `local-llm/src/preset.rs`
- [x] T012 [P] [US1] Write test `gemma4_e4b_preset_config_defaults` — verify repo_id, filename, context_length (131072) for `ModelPreset::Gemma4_E4B` in `local-llm/src/preset.rs`
- [x] T013 [P] [US1] Write test `gemma4_26b_preset_config_defaults` — verify repo_id, filename, context_length (262144) for `ModelPreset::Gemma4_26B` in `local-llm/src/preset.rs`
- [x] T014 [P] [US1] Write test `gemma4_e2b_env_override` — verify `LOCAL_MODEL_REPO`/`LOCAL_MODEL_FILE` env vars override Gemma 4 E2B defaults in `local-llm/src/preset.rs`
- [x] T015 [P] [US1] Write test `all_presets_includes_gemma4_variants` — verify `ModelPreset::all()` returns all 5 variants when `gemma4` feature enabled in `local-llm/src/preset.rs`
- [x] T016 [P] [US1] Write test `gemma4_builder_uses_multimodal` — verify `ChatBackend::build()` uses `MultimodalModelBuilder` when `config.is_gemma4()` is true in `local-llm/src/model.rs` (compilation test — verify the code path compiles, not a live inference test)

### Implementation for User Story 1

- [x] T017 [P] [US1] Add `ModelPreset::Gemma4_E2B`, `Gemma4_E4B`, `Gemma4_26B` variants behind `#[cfg(feature = "gemma4")]` in `local-llm/src/preset.rs` — implement `config()`, `embedding_config()`, `Display`, update `all()`
- [x] T018 [P] [US1] Add `MultimodalModelBuilder` branch in `ChatBackend::build()` behind `#[cfg(feature = "gemma4")]` in `local-llm/src/model.rs` — branch on `config.is_gemma4()`, use `MultimodalModelBuilder::new(repo_id, vec![filename]).build()`
- [x] T019 [US1] Update `local-llm/src/lib.rs` — add cfg-gated re-exports for new `ModelPreset` variants
- [x] T020 [US1] Verify all T011-T016 tests pass with `--features gemma4`

**Checkpoint**: Gemma 4 E2B can be loaded and run via direct inference (text streaming works)

### Validation Gate (STOP/GO)

- [ ] T021 Run ignored live test `live_gemma4_e2b_smoke` — download E2B Q4_K_M GGUF, send 3 prompts of increasing complexity (simple greeting, multi-paragraph system prompt, tool-use-style prompt), verify no NaN/hang/garbage output in `local-llm/tests/local_live.rs`
- **If PASS**: Continue to Phase 4+. E2B is confirmed safe with Q4_K_M GGUF on mistralrs 0.8.0.
- **If FAIL (NaN)**: STOP. Pause all further phases. Fall back to Ollama path until upstream fix. File upstream issue with reproduction details.

---

## Phase 4: User Story 2 — Thinking Mode in Direct Inference (Priority: P1)

**Goal**: When thinking is enabled for Gemma 4, the system injects `<|think|>` into the system prompt and correctly parses `<|channel>thought\n...<channel|>` output into ThinkingStart/Delta/End events, including across chunk boundaries.

**Independent Test**: Send a prompt with thinking enabled, verify ThinkingStart/ThinkingDelta/ThinkingEnd events are emitted with correct content.

### Tests for User Story 2

- [ ] T022 [P] [US2] Write test `channel_thought_single_chunk` — full `<|channel>thought\nreasoning here<channel|>` in one chunk produces ThinkingStart + ThinkingDelta + ThinkingEnd in `local-llm/src/stream.rs`
- [ ] T023 [P] [US2] Write test `channel_thought_cross_chunk_open` — opening delimiter `<|channel>thought\n` split across two chunks, parser reassembles correctly in `local-llm/src/stream.rs`
- [ ] T024 [P] [US2] Write test `channel_thought_cross_chunk_close` — closing delimiter `<channel|>` split across two chunks, parser reassembles correctly in `local-llm/src/stream.rs`
- [ ] T025 [P] [US2] Write test `channel_thought_no_delimiters` — plain text without delimiters emits only text events, no thinking events in `local-llm/src/stream.rs`
- [ ] T026 [P] [US2] Write test `channel_thought_multiple_blocks` — two consecutive thinking blocks in a single response each produce separate ThinkingStart/Delta/End sequences in `local-llm/src/stream.rs`
- [ ] T027 [P] [US2] Write test `channel_thought_mixed_text_and_thinking` — text before, thinking block, text after — emits TextDelta, ThinkingStart/Delta/End, TextDelta in correct order in `local-llm/src/stream.rs`
- [ ] T028 [P] [US2] Write test `channel_thought_delimiter_in_text` — input `"The format is <|channel>thought"` as quoted explanation text should NOT trigger thinking events, only emit text in `local-llm/src/stream.rs`
- [ ] T029 [P] [US2] Write test `think_token_injected_for_gemma4` — `convert_context_messages()` with Gemma 4 config and `thinking_enabled: true` prepends `<|think|>\n` to system prompt in `local-llm/src/convert.rs`
- [ ] T030 [P] [US2] Write test `think_token_not_injected_for_smollm` — `convert_context_messages()` with SmolLM3 config does NOT inject `<|think|>` in `local-llm/src/convert.rs`
- [ ] T031 [P] [US2] Write test `think_token_not_injected_when_thinking_disabled` — `convert_context_messages()` with Gemma 4 config but `thinking_enabled: false` does NOT inject `<|think|>` in `local-llm/src/convert.rs`

### Implementation for User Story 2

- [ ] T032 [P] [US2] Implement `ChannelThoughtParser` struct with 4-state machine (`Normal`, `InThinking`, `PartialOpen`, `PartialClose`) behind `#[cfg(feature = "gemma4")]` in `local-llm/src/stream.rs` — `fn process(&mut self, content: &str) -> (Option<String>, String)` method
- [ ] T033 [US2] Change `StreamState::new()` to accept `is_gemma4: bool` parameter; add `ChannelThoughtParser` as optional field on `StreamState` behind `#[cfg(feature = "gemma4")]` in `local-llm/src/stream.rs` — initialize parser when `is_gemma4` is true
- [ ] T034 [US2] Update `StreamState::process_content_delta()` to dispatch to `ChannelThoughtParser` when present (Gemma 4) or `extract_thinking_delta()` when absent (SmolLM3-3B) in `local-llm/src/stream.rs`
- [ ] T035 [US2] Update `convert_context_messages()` signature to accept `config: &ModelConfig` and `thinking_enabled: bool` parameters in `local-llm/src/convert.rs` — inject `<|think|>\n` prefix on system prompt when `config.is_gemma4()` and `thinking_enabled` is true
- [ ] T036 [US2] Update `local_stream()` to extract `thinking_enabled` from `ModelSpec.capabilities.supports_thinking`, pass `ModelConfig` and `thinking_enabled` to `convert_context_messages()`, and pass `is_gemma4` to `StreamState::new()` in `local-llm/src/stream.rs`
- [ ] T037 [US2] Verify all T022-T031 tests pass with `--features gemma4`

**Checkpoint**: Gemma 4 thinking mode works end-to-end in direct inference with cross-chunk parsing

---

## Phase 5: User Story 3 — Backward-Compatible Default Swap (Priority: P2)

**Goal**: Default local model changes from SmolLM3-3B to Gemma 4 E2B when `gemma4` feature is enabled. SmolLM3-3B remains default when feature is disabled. Env var overrides work.

**Independent Test**: Check default preset resolves to Gemma 4 E2B with feature enabled, SmolLM3-3B with feature disabled.

### Tests for User Story 3

- [ ] T038 [P] [US3] Write test `default_preset_is_gemma4_e2b` — `DEFAULT_LOCAL_PRESET_ID` equals `"gemma4_e2b"` when `gemma4` feature enabled in `local-llm/src/preset.rs`
- [ ] T039 [P] [US3] Write test `default_local_connection_uses_gemma4` — `default_local_connection()` returns a connection with Gemma 4 E2B model spec when `gemma4` feature enabled in `local-llm/src/preset.rs`
- [ ] T040 [P] [US3] Write test `smollm3_preset_still_available` — `ModelPreset::SmolLM3_3B.config()` returns correct SmolLM3 config regardless of feature flag in `local-llm/src/preset.rs`

### Implementation for User Story 3

- [ ] T041 [US3] Change `DEFAULT_LOCAL_PRESET_ID` to `"gemma4_e2b"` behind `#[cfg(feature = "gemma4")]` with fallback to `"smollm3_3b"` when feature disabled in `local-llm/src/preset.rs`
- [ ] T042 [US3] Update `ModelConfig::default()` to resolve context_length from the new default preset (131072 for Gemma 4 E2B vs 8192 for SmolLM3) in `local-llm/src/model.rs`
- [ ] T043 [US3] Verify all T038-T040 tests pass with `--features gemma4`
- [ ] T044 [US3] Verify `cargo test -p swink-agent-local-llm` (without `gemma4` feature) — all existing SmolLM3 tests still pass unchanged

**Checkpoint**: Default swap works, backward compatibility preserved

---

## Phase 6: User Story 4 — Alternative Backends Documentation (Priority: P2)

**Goal**: Documentation guiding developers to run Gemma 4 E2B via llama.cpp server, vLLM, or LM Studio using the existing OpenAI-compatible adapter.

**Independent Test**: Follow documented setup steps for any alternative backend, verify streaming inference works.

### Implementation for User Story 4

- [ ] T045 [US4] Write alternative backends documentation section in `specs/041-gemma4-local-default/quickstart.md` — expand existing llama.cpp, vLLM, LM Studio sections with full step-by-step instructions including model download, server start, and adapter configuration
- [ ] T046 [US4] Document known limitations per backend in quickstart.md — LM Studio streaming+tools bug (#1066), vLLM `reasoning_content` field behavior

**Checkpoint**: Developers have clear documentation for all Gemma 4 backend options

---

## Phase 7: User Story 5 — Tool Calling in Direct Inference (Priority: P3)

**Goal**: Parse Gemma 4's native tool call format (`<|tool_call>call:{name}{args}<tool_call|>`) from raw model output and emit standard ToolCallStart/Delta/End events.

**Independent Test**: Send a prompt that triggers a tool call, verify tool call events are emitted with correct function name and arguments.

**Depends on**: US1 (core inference) and US2 (thinking mode) must be stable first.

### Tests for User Story 5

- [ ] T047 [P] [US5] Write test `tool_call_single_chunk` — full `<|tool_call>call:read_file{"path":"foo.rs"}<tool_call|>` in one chunk produces ToolCallStart + ToolCallDelta + ToolCallEnd in `local-llm/src/stream.rs`
- [ ] T048 [P] [US5] Write test `tool_call_cross_chunk` — tool call delimiter split across chunks, parser reassembles correctly in `local-llm/src/stream.rs`
- [ ] T049 [P] [US5] Write test `tool_call_no_delimiters` — plain text without tool call markers emits only text events in `local-llm/src/stream.rs`
- [ ] T050 [P] [US5] Write test `tool_call_with_thinking` — response contains both thinking block and tool call, both parsed correctly in `local-llm/src/stream.rs`
- [ ] T051 [P] [US5] Write test `tool_result_formatting` — `LocalConverter::tool_result_message()` formats result in `<|tool_result>{name}\n{text}<tool_result|>` format for Gemma 4 in `local-llm/src/convert.rs`

### Implementation for User Story 5

- [ ] T052 [P] [US5] Implement `ToolCallParser` struct with stateful delimiter parsing behind `#[cfg(feature = "gemma4")]` in `local-llm/src/stream.rs` — extracts function name and JSON arguments from `<|tool_call>call:{name}{args}<tool_call|>` format
- [ ] T053 [US5] Integrate `ToolCallParser` into `StreamState` — dispatch to parser when Gemma 4 model detected, alongside existing `process_tool_call_delta()` for mistralrs-native tool calls in `local-llm/src/stream.rs`
- [ ] T054 [US5] Update `LocalConverter::tool_result_message()` to format results as `<|tool_result>{name}\n{text}<tool_result|>` when model is Gemma 4 in `local-llm/src/convert.rs`
- [ ] T055 [US5] Verify all T047-T051 tests pass with `--features gemma4`

**Checkpoint**: Tool calling works end-to-end in direct Gemma 4 inference

---

## Phase 8: Polish & Cross-Cutting Concerns

**Purpose**: Verification, documentation updates, and cross-story validation

- [ ] T056 [P] Add ignored live test `live_gemma4_e2b_text_stream` — download E2B model and stream text in `local-llm/tests/local_live.rs`
- [ ] T057 [P] Add ignored live test `live_gemma4_e2b_thinking` — verify thinking events with real model in `local-llm/tests/local_live.rs`
- [ ] T058 [P] Add ignored live test `live_gemma4_e2b_tool_call` — verify tool calling with real model in `local-llm/tests/local_live.rs`
- [ ] T059 Run `cargo clippy -p swink-agent-local-llm --features gemma4 -- -D warnings` — zero warnings
- [ ] T060 Run `cargo test --workspace --features testkit` — verify no regressions across workspace
- [ ] T061 Run `cargo build -p swink-agent-local-llm --no-default-features` — verify builds without gemma4 feature (SmolLM3-3B only)
- [ ] T062 Update `local-llm/CLAUDE.md` — document Gemma 4 default change, thinking behavior, MultimodalModelBuilder branching, feature gate
- [ ] T063 Update lib.rs doc comment in `local-llm/src/lib.rs` — add Gemma 4 E2B to the crate-level documentation alongside SmolLM3-3B

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories
- **US1 (Phase 3)**: Depends on Foundational — core inference
- **Validation Gate (T021)**: After US1, run live E2B smoke test. STOP/GO decision point.
- **US2 (Phase 4)**: Depends on Foundational + validation gate pass — can run in parallel with US1 tests (different files: stream.rs parser vs preset.rs/model.rs builder)
- **US3 (Phase 5)**: Depends on US1 (preset variants must exist before changing default)
- **US4 (Phase 6)**: No code dependencies — can run in parallel with any phase (documentation only)
- **US5 (Phase 7)**: Depends on US1 and US2 being stable (tool calling builds on core inference + parser infrastructure)
- **Polish (Phase 8)**: Depends on all user stories complete

### User Story Dependencies

```
Phase 1 (Setup)
    |
Phase 2 (Foundational: is_gemma4)
    |
    +-- Phase 3 (US1: Presets + Builder)
    |       |
    |       T021 (Validation Gate: STOP/GO)
    |       |
    |       +-- Phase 5 (US3: Default Swap)
    +-- Phase 4 (US2: Thinking Parser) --+
    |                                     +-- Phase 7 (US5: Tool Calling)
    +-- Phase 6 (US4: Docs) -- [independent]
```

### Parallel Opportunities

**Within Phase 2**: T006, T007, T008 (tests) can run in parallel
**Within Phase 3**: T011-T016 (tests) in parallel; T017, T018 (impl) in parallel (different files)
**Within Phase 4**: T022-T031 (tests) in parallel; T032 can run alongside T035 (different files: stream.rs vs convert.rs)
**US1 and US2**: Can run in parallel — US1 touches `preset.rs` + `model.rs` builder; US2 touches `stream.rs` parser + `convert.rs`
**US4**: Can run any time — documentation only, no code dependencies

---

## Parallel Example: User Story 1 + User Story 2

```text
# After Phase 2 completes, launch US1 and US2 in parallel:

# US1 agent (preset.rs + model.rs):
T011-T016 (tests in parallel) → T017, T018 (impl in parallel) → T019 → T020

# US2 agent (stream.rs + convert.rs):
T022-T031 (tests in parallel) → T032, T035 (impl in parallel) → T033 → T034 → T036 → T037
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup (mistralrs 0.8 + feature flag)
2. Complete Phase 2: Foundational (`is_gemma4()` detection)
3. Complete Phase 3: User Story 1 (presets + builder branching)
4. **STOP and VALIDATE**: Run live test with real Gemma 4 E2B model
5. Text streaming works → MVP ready

### Incremental Delivery

1. Setup + Foundational → mistralrs 0.8 integrated
2. Add US1 (core inference) → Gemma 4 text streaming works
3. Add US2 (thinking mode) → Thinking events parsed correctly
4. Add US3 (default swap) → Gemma 4 E2B is the default
5. Add US4 (docs) → Alternative backends documented
6. Add US5 (tool calling) → Full feature parity with Ollama path

### Parallel Team Strategy

With two developers after Foundational phase:
- Developer A: US1 (preset.rs, model.rs builder) → US3 (default swap)
- Developer B: US2 (stream.rs parser, convert.rs injection) → US5 (tool calling)
- US4 (docs) can be done by either developer at any point

---

## Notes

- All Gemma 4 code is behind `#[cfg(feature = "gemma4")]` — zero impact when feature disabled
- Tests use `#[cfg(feature = "gemma4")]` gating to avoid compilation when feature is off
- Live tests (T056-T058) download ~3.5 GB on first run — always `#[ignore]`
- `ChannelThoughtParser` and `ToolCallParser` follow the same stateful pattern — implement thinking first, tool calling reuses the pattern
- `convert_context_messages()` signature change (T035) requires updating call site in `stream.rs` (T036) — these must be sequential
