# Research: Gemma 4 Local Default (Direct Inference)

**Feature**: 041-adapter-gemma4-local  
**Date**: 2026-04-04

## R1: mistral.rs 0.7 → 0.8 Migration

**Decision**: Upgrade `mistralrs` from 0.7 to 0.8.0. Proceed with E2B (dense architecture, Q4_K_M GGUF) implementation immediately; defer E4B/26B (MoE) until upstream MoE fix ships.

**Rationale**: Version 0.7 does not support Gemma 4 architecture at all. Version 0.8.0 (released 2026-04-02) adds Gemma 4 support. The NaN logits bug (#2051) was reported against MoE variants (26B-A4B, E4B) with BF16 and UQFF quantization. E2B is a dense (non-MoE) architecture and uses Q4_K_M GGUF — a different code path from the affected models. A post-release commit on `main` ("Fixes for MoE Gemma 4 variants") exists but is not in any tagged release. E2B implementation proceeds with a live validation gate; MoE variants wait for the fix release.

**Alternatives considered**:
- Stay on 0.7 and only use Ollama: Rejected — the spec requires direct inference without Ollama.
- Fork mistral.rs and fix NaN bug locally: Rejected — maintenance burden too high; wait for upstream fix.
- Use candle directly instead of mistral.rs: Rejected — mistral.rs wraps candle with chat template handling, tokenization, and streaming; reimplementing this is not justified.

**Key findings**:
- `GgufModelBuilder` cannot load Gemma 4 (Per-Layer Embeddings architecture unsupported).
- `MultimodalModelBuilder` is required even for text-only Gemma 4 inference.
- The `mistralrs::Model` return type is the same for both builders — downstream code (loader, stream) does not change.
- `stream_chat_request` and `ChatCompletionChunkResponse` APIs appear stable across 0.7 → 0.8.

## R2: Model Builder Branching Strategy

**Decision**: Branch in `ChatBackend::build()` based on model-family detection via `ModelConfig::is_gemma4()`.

**Rationale**: The `LoaderBackend` trait's `build()` method is the only place where the builder is constructed. Branching here keeps the change minimal — one `if/else` in a single function. The `LazyLoader` state machine, `LoaderState`, `LocalStreamFn`, and all consumer code remain unchanged because both builders produce the same `mistralrs::Model` type.

**Alternatives considered**:
- Separate `GemmaBackend` struct implementing `LoaderBackend`: Rejected — adds a new type, a new `LazyLoader<GemmaBackend>` instantiation, and a runtime dispatch at `LocalModel` construction. The branching is one builder call; a whole new backend type is overengineered.
- Generic over builder type: Rejected — `GgufModelBuilder` and `MultimodalModelBuilder` have no shared trait; generic abstraction would require a custom trait that just wraps the two builders.

**Key findings**:
- `ModelConfig::is_gemma4()` checks `repo_id` for `"gemma-4"` or `"gemma4"` substrings. This covers all known Gemma 4 GGUF repositories (bartowski, unsloth, ggml-org).
- `MultimodalModelBuilder::new(repo_id, vec![filename])` has the same constructor signature as `GgufModelBuilder::new(repo_id, vec![filename])`.
- Both return `Result<Model, _>` from `.build().await`.

## R3: Gemma 4 Thinking Output Format

**Decision**: Add a stateful `ChannelThoughtParser` that handles `<|channel>thought\n...<channel|>` delimiters with cross-chunk boundary support. Select parser based on model family.

**Rationale**: The existing `extract_thinking_delta()` is a stateless single-chunk parser for `<think>...</think>`. Gemma 4's delimiters are different AND can split across streaming chunks. A stateful parser that buffers partial delimiter matches is required.

**Alternatives considered**:
- Try both delimiter formats in a single parser: Rejected — the two formats have no structural overlap; combining them adds complexity without benefit. Better to dispatch once based on model family.
- Use regex: Rejected — regex adds overhead per chunk on a hot path. Simple string matching with a state machine is faster and sufficient.
- Parse only within single chunks (like existing `<think>` parser): Rejected — spec explicitly requires cross-chunk boundary handling (FR-005).

**Key findings**:
- Opening delimiter: `<|channel>thought\n` (literal `<|channel>thought` followed by newline).
- Closing delimiter: `<channel|>`.
- Content between delimiters is the thinking text.
- Multiple thinking blocks possible per response (each opened/closed independently).
- The parser needs three states: `Normal` (emitting text), `InThinking` (emitting thinking deltas), `PartialDelimiter` (buffering characters that might be a delimiter prefix).

## R4: System Prompt `<|think|>` Injection

**Decision**: Inject `<|think|>` token as the first line of the system prompt when thinking is enabled for Gemma 4 in direct inference.

**Rationale**: mistral.rs has no `think: true` API like Ollama does. The `<|think|>` control token must be injected manually into the prompt. Placing it at the start of the system prompt (before the user's system content) ensures the model enters thinking mode for the entire conversation.

**Alternatives considered**:
- Append `<|think|>` at the end of system prompt: Rejected — Gemma 4 documentation specifies it should precede the conversation.
- Inject via chat template override: Rejected — requires maintaining a custom Jinja template; fragile across model versions.
- Skip thinking injection for direct inference: Rejected — spec requires thinking support (FR-004, User Story 2).

**Key findings**:
- The injection point is `LocalConverter::system_message()` in `convert.rs`.
- Only inject when: (1) model is Gemma 4 AND (2) thinking is enabled in `StreamOptions` or model config.
- `StreamOptions` currently does not carry thinking-level information — this needs to be threaded from the `ModelSpec` capabilities or passed explicitly.

## R5: Gemma 4 Variant GGUF Sources

**Decision**: Use bartowski GGUF repositories for all three variants with Q4_K_M quantization as default.

**Rationale**: bartowski provides consistent, well-tested GGUF quantizations for all Gemma 4 sizes. Apache 2.0 licensed, ungated, no HF_TOKEN required.

| Variant | Repository | Filename | Size (Q4_K_M) | Context |
|---------|-----------|----------|----------------|---------|
| E2B | `bartowski/google_gemma-4-E2B-it-GGUF` | `gemma-4-E2B-it-Q4_K_M.gguf` | ~3.46 GB | 128K |
| E4B | `bartowski/google_gemma-4-E4B-it-GGUF` | `gemma-4-E4B-it-Q4_K_M.gguf` | ~5.5 GB | 128K |
| 26B | `bartowski/google_gemma-4-26B-it-GGUF` | `gemma-4-26B-it-Q4_K_M.gguf` | ~16 GB | 256K |

## R6: Gemma 4 Native Tool Call Format

**Decision**: Parse `<|tool_call>call:{function_name}{json_args}<tool_call|>` format in a dedicated tool call parser, implemented as P3 after core inference is stable.

**Rationale**: Gemma 4 uses a custom serialization for tool calls that differs from both OpenAI function calling format and the simple JSON format used by other models. In Ollama, this is abstracted away — the adapter sees standard JSON tool calls. In direct inference, the raw model output contains these custom tokens.

**Key findings**:
- Tool call format: `<|tool_call>call:{function_name}{json_arguments}<tool_call|>`
- Multiple tool calls in a single response are possible.
- Tool results are formatted as: `<|tool_result>{function_name}\n{result_text}<tool_result|>`
- The parser needs to extract function name and JSON arguments from the raw token stream.
- This is separate from the thinking parser — tool calls and thinking can coexist in the same response.

## R7: Feature Gate Strategy

**Decision**: Add a `gemma4` feature flag to the `local-llm` crate. When disabled, the `MultimodalModelBuilder` code path, `ChannelThoughtParser`, `<|think|>` injection, and all three Gemma 4 presets are excluded from compilation.

**Rationale**: The upstream inference engine's Gemma 4 support is unstable (NaN bug). Feature-gating allows shipping the code as opt-in until stable. Users who don't need Gemma 4 in direct inference pay zero compile-time or binary-size cost.

**Key findings**:
- Gate pattern follows `swink-agent-adapters` and `swink-agent-policies`: paired `#[cfg(feature)]` on `mod` + `pub use`.
- The `gemma4` feature may need to forward to a `mistralrs` feature if 0.8 introduces Gemma-4-specific compilation flags.
- Default preset change (SmolLM3-3B → Gemma 4 E2B) should also be behind the feature gate — when `gemma4` is disabled, SmolLM3-3B remains the default.
