# Feature Specification: Gemma 4 Local Inference (Opt-In Direct)

**Feature Branch**: `041-adapter-gemma4-local`  
**Created**: 2026-04-04  
**Status**: Draft  
**Input**: Add first-class Gemma 4 model family support to swink-agent, making Gemma 4 E2B the default local/edge model via direct in-process inference (Phase 3). Phases 1-2 (Ollama path, catalog presets) are already complete.

## Clarifications

### Session 2026-04-04

- Q: Does Phase 3 (direct inference) include tool calling support, or only text + thinking? → A: Tool calling included as a separate P3 user story, implemented after core text + thinking inference is stable.
- Q: Are E4B and 26B variants in scope for direct local inference, or only E2B? → A: All three variants (E2B, E4B, 26B) are in scope for direct inference.
- Q: What is the maximum thinking content size before truncation? → A: No hard limit. Thinking content streams incrementally via ThinkingDelta events without full-block accumulation. Consumers (agent loop, TUI) decide truncation policy, not the parser.

## Prior Art

The following work is already complete and should not be re-implemented:

- **Catalog presets** (`gemma4_e2b`, `gemma4_e4b`, `gemma4_26b`) added to `src/model_catalog.toml`
- **Ollama adapter** `think` field wired to send `think: true` when `thinking_level != Off`
- **SmolLM3-3B** demoted to legacy in the catalog (`include_by_default = false`, `group = "legacy"`)
- **Spec addenda** on specs 008-model-catalog-presets, 014-adapter-ollama, 022-local-llm-crate
- All existing tests pass

This specification covers **Phase 3: Direct Local Inference** — running Gemma 4 E2B in-process via the local-llm crate without requiring Ollama.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Run Gemma 4 E2B Locally Without Ollama (Priority: P1)

A developer with no Ollama installation wants to use Gemma 4 E2B as the default local model. They configure the agent to use the local-llm crate. On first use, the model weights (~3.5 GB Q4_K_M quantization) download automatically from HuggingFace and cache locally. Subsequent uses load from cache. The model provides 128K context with streaming inference, delivering a substantially better local experience than the previous SmolLM3-3B default (8K context, no thinking, no tools).

**Why this priority**: This is the core deliverable — direct local inference is the only reason this spec exists. Without it, users must install Ollama or use a separate server process.

**Independent Test**: Can be tested by configuring the local-llm crate with default settings, sending a prompt, and verifying that text tokens stream back incrementally without any external service dependency.

**Acceptance Scenarios**:

1. **Given** a fresh system with no cached models and no Ollama installed, **When** a developer uses the local-llm crate with default settings, **Then** the Gemma 4 E2B model downloads from HuggingFace (~3.5 GB), caches in the standard cache directory, and produces streaming text output.
2. **Given** the Gemma 4 E2B model is already cached, **When** inference is requested, **Then** the model loads from cache without downloading, and streaming begins within a reasonable startup time.
3. **Given** a developer previously relied on SmolLM3-3B as the default, **When** they upgrade to this version, **Then** the default model changes to Gemma 4 E2B transparently, and SmolLM3-3B remains available as an override.
4. **Given** a developer prefers SmolLM3-3B, **When** they set environment variable overrides for model repository and filename, **Then** the system uses their specified model instead of Gemma 4 E2B.

---

### User Story 2 - Thinking Mode in Direct Inference (Priority: P1)

A developer enables thinking mode for Gemma 4 E2B in the local-llm crate. The system activates the model's built-in reasoning capability by injecting the appropriate control token into the system prompt (since direct inference has no higher-level API like Ollama's `think: true`). The model's thinking output uses a unique delimiter format (`<|channel>thought\n...<channel|>`) that differs from other models' `<think>...</think>` format. The system correctly parses this format — including when delimiters span multiple streaming chunks — and emits standard thinking events that the rest of the agent framework can consume.

**Why this priority**: Thinking mode is a defining capability of Gemma 4 over SmolLM3-3B. Without it, direct inference loses a key advantage that users already get via the Ollama path.

**Independent Test**: Can be tested by sending a prompt with thinking enabled, capturing the streaming events, and verifying that ThinkingStart, ThinkingDelta, and ThinkingEnd events are emitted with the correct thinking content extracted from the model's output.

**Acceptance Scenarios**:

1. **Given** thinking mode is enabled for Gemma 4 E2B in direct inference, **When** a prompt is sent, **Then** the system injects the thinking control token into the system prompt before passing it to the inference engine.
2. **Given** the model produces output with thinking delimiters, **When** the output is streamed, **Then** the system emits ThinkingStart, ThinkingDelta, and ThinkingEnd events with the extracted thinking content.
3. **Given** the model produces thinking delimiters that span across two or more streaming chunks, **When** the chunks arrive sequentially, **Then** the parser correctly reassembles the delimiters and emits accurate thinking events without data loss or corruption.
4. **Given** the model produces output without thinking delimiters, **When** the output is streamed, **Then** the system emits only text content events with no spurious thinking events.

---

### User Story 3 - Opt-In Gemma 4 for Capable Hardware (Priority: P2)

A developer with sufficient hardware (16+ GB RAM) wants to use Gemma 4 E2B for its superior capabilities (128K context, thinking, tools) instead of the default SmolLM3-3B (8K context). They select Gemma 4 E2B via the preset API or environment variables. SmolLM3-3B remains the default for all users — no Ollama dependency is introduced. The system never requires an external service for the default inference path.

**Why this priority**: Opt-in Gemma 4 selection depends on the core inference (Story 1) and thinking (Story 2) working correctly first.

**Independent Test**: Can be tested by selecting Gemma 4 E2B via preset, verifying it loads correctly, and confirming the default still resolves to SmolLM3-3B when no override is set.

**Acceptance Scenarios**:

1. **Given** a new installation with default settings, **When** the local-llm crate resolves the default model, **Then** it selects SmolLM3-3B (unchanged).
2. **Given** a developer explicitly selects `ModelPreset::Gemma4E2B`, **When** the model is loaded, **Then** Gemma 4 E2B is used with 128K context and thinking support.
3. **Given** environment variables override the model to Gemma 4, **When** the default model is resolved, **Then** the overridden Gemma 4 model is used.
4. **Given** the `gemma4` feature flag is disabled, **When** the crate is compiled, **Then** Gemma 4 presets are not available and SmolLM3-3B is the only chat preset.

---

### User Story 4 - Alternative Backends for Gemma 4 (Priority: P2)

A developer who cannot or does not want to use either the local-llm crate or Ollama can run Gemma 4 E2B via alternative inference servers (llama.cpp server, vLLM, LM Studio). These servers expose standard chat completion APIs that work with the existing adapter infrastructure with zero code changes. Documentation guides the developer through setup and connection for each backend.

**Why this priority**: This provides flexibility for users with different infrastructure preferences and serves as a fallback while the direct inference path matures.

**Independent Test**: Can be tested by starting any of the documented alternative servers, pointing the agent at it, and verifying that streaming text, thinking, and tool calling work end-to-end.

**Acceptance Scenarios**:

1. **Given** a developer runs Gemma 4 E2B via llama.cpp server, **When** they configure the agent to use the server endpoint, **Then** streaming inference, thinking output, and tool calling work correctly.
2. **Given** a developer runs Gemma 4 E2B via vLLM, **When** they configure the agent to use the vLLM endpoint, **Then** streaming inference and tool calling work correctly.
3. **Given** documentation exists for each alternative backend, **When** a developer follows the setup instructions, **Then** they achieve a working Gemma 4 E2B configuration within the documented steps.

---

### User Story 5 - Tool Calling in Direct Inference (Priority: P3)

A developer uses Gemma 4 E2B via direct local inference and wants tool calling support. The system parses Gemma 4's custom tool call output format from the raw model output and emits standard tool call events that the agent framework can dispatch. This is implemented after the core inference (Story 1) and thinking mode (Story 2) are stable and working.

**Why this priority**: Tool calling via direct inference adds significant parsing complexity for a model-specific serialization format. The Ollama path (Phase 1-2, already complete) already provides full tool calling support, so this is an incremental improvement rather than a blocking capability. Core text + thinking must be stable first.

**Independent Test**: Can be tested by sending a prompt that triggers a tool call, capturing the streaming events, and verifying that tool call events are emitted with correct function name and arguments parsed from Gemma 4's native tool call format.

**Acceptance Scenarios**:

1. **Given** Gemma 4 E2B is running via direct inference with tools registered, **When** a prompt triggers a tool call, **Then** the system parses the model's native tool call format and emits standard tool call events.
2. **Given** the model produces a tool call response, **When** the tool result is returned, **Then** the system formats the result for the next inference turn and the model can continue the conversation.
3. **Given** the model produces a response with no tool calls, **When** tools are registered, **Then** the system emits only text/thinking events with no spurious tool call events.

---

### Edge Cases

- What happens when the upstream inference engine (mistral.rs) produces NaN or infinite values during logit computation? The system should detect numerical instability and return a clear error rather than producing garbage output or hanging indefinitely.
- What happens when the model download is interrupted mid-transfer? The system should resume or restart the download cleanly on the next attempt.
- What happens when thinking delimiters appear in the model's regular text output (e.g., the model describes its own delimiter format in a response)? The parser should only activate on actual thinking channel markers, not on escaped or quoted references.
- What happens when the model produces an extremely long thinking block? The thinking parser streams content incrementally via ThinkingDelta events without accumulating the full block in memory. No hard size limit is enforced at the parser level — consumers (agent loop, TUI) apply their own truncation policies.
- What happens when a user sets conflicting environment variables (e.g., a SmolLM3-3B repo with Gemma 4 E2B filename)? The system should attempt to load the specified configuration and surface any resulting errors clearly.
- **Malformed `<think>` delimiters**: The thinking-mode parser handles malformed delimiter sequences robustly: unclosed `<think>` tags (no matching `</think>`) are treated as plain text rather than thinking content; stray `</think>` delimiters without a matching opening tag are ignored. Neither case produces spurious thinking events.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide presets for all three Gemma 4 variants (E2B, E4B, 26B) in the local-llm crate, each bundling the correct HuggingFace repository, model weights, and context window configuration (128K tokens for E2B/E4B, 256K for 26B). Gemma 4 presets are opt-in — SmolLM3-3B remains the default.
- **FR-002**: System MUST automatically download Gemma 4 E2B model weights (~3.5 GB Q4_K_M) from HuggingFace on first use, caching them in the standard local cache directory for subsequent use.
- **FR-003**: System MUST use the appropriate model builder for Gemma 4 architecture, which differs from the builder used for existing models. Model-family detection determines which builder to use based on model configuration.
- **FR-004**: System MUST inject the thinking control token into the system prompt when thinking mode is enabled for Gemma 4 in direct inference, since the inference engine provides no higher-level thinking API.
- **FR-005**: System MUST parse Gemma 4's thinking output delimiters (`<|channel>thought\n...<channel|>`) and emit standard ThinkingStart, ThinkingDelta, and ThinkingEnd events, correctly handling delimiters that span multiple streaming chunks.
- **FR-006**: System MUST keep SmolLM3-3B as the default local model preset. Gemma 4 E2B is available as an opt-in preset for machines with sufficient hardware (16+ GB RAM). Users select Gemma 4 via `ModelPreset::Gemma4E2B` or environment variable override — the agent MUST NOT depend on Ollama for any default or fallback path.
- **FR-007**: System MUST gate all Gemma 4 direct inference changes behind a feature flag until the upstream inference engine stabilizes support. The feature flag controls whether Gemma 4-specific code paths are compiled.
- **FR-008**: System MUST validate E2B inference via a live test before shipping. The known upstream NaN logits bug (#2051) was reported against MoE variants (26B) with BF16/UQFF quantization — E2B (dense architecture) may not be affected. If E2B live validation passes, it can ship. E4B and 26B (MoE) presets MUST remain behind the feature flag until the upstream MoE fix is released.
- **FR-009**: System MUST support model-family detection that determines whether a given model configuration is Gemma 4, enabling family-specific code paths (builder selection, thinking token injection, output parsing) without hardcoding to a single model.
- **FR-010**: System MUST preserve full backward compatibility for existing SmolLM3-3B users — the SmolLM3-3B preset, its configuration, and its inference behavior remain available and unchanged.
- **FR-011**: System MUST parse Gemma 4's native tool call output format in direct inference and emit standard tool call events. This requirement is P3 priority — implemented only after core text generation (FR-001 through FR-003) and thinking mode (FR-004, FR-005) are stable.

### Key Entities

- **ModelPreset**: Named configuration bundle for a local model. Gains three Gemma 4 variants (E2B, E4B, 26B) alongside the existing SmolLM3-3B and EmbeddingGemma-300M variants. Each preset specifies repository, filename, context window, and builder type. SmolLM3-3B remains the default; Gemma 4 variants are opt-in for capable hardware.
- **Thinking Parser**: Component that extracts thinking content from model output. Must support multiple delimiter formats (existing `<think>...</think>` for SmolLM3-3B, new `<|channel>thought\n...<channel|>` for Gemma 4) with cross-chunk boundary handling.
- **Model Builder**: Abstraction over the inference engine's model construction. Gemma 4 requires a different builder type than existing models, introducing the first model-family-specific branching in the local-llm crate.

## Assumptions

- The upstream NaN logits bug (#2051) affects MoE variants (26B) with BF16/UQFF quantization. E2B (dense architecture) with Q4_K_M GGUF may not be affected — validated via live test before shipping. If E2B also exhibits NaN, implementation pauses and the Ollama path (already complete) serves as the primary local inference method.
- The GGUF quantization source (`bartowski/google_gemma-4-E2B-it-GGUF`, Q4_K_M) will remain available on HuggingFace under Apache 2.0 license, ungated, requiring no authentication token.
- The inference engine's API changes between versions 0.7 and 0.8 are manageable and do not fundamentally alter the local-llm crate's architecture beyond the documented builder change.
- 128K context window is the correct default for Gemma 4 E2B. Users who need shorter context (to reduce memory usage) can override via environment variable.
- The `<|channel>thought\n...<channel|>` delimiter format is stable and will not change in future Gemma 4 releases.

## Dependencies

- **Upstream inference engine v0.8+**: Required for Gemma 4 architecture support. Current v0.7 does not support Gemma 4 at all. v0.8.0 is released; a post-release MoE fix exists on `main` but is not tagged. E2B (dense) can proceed on v0.8.0. E4B/26B (MoE) await the fix release.
- **Prior art** (complete): Catalog presets (spec 008), Ollama adapter thinking support (spec 014), local-llm crate architecture (spec 022).
- **HuggingFace model hosting**: Gemma 4 E2B GGUF weights must remain publicly available.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A developer with no Ollama installation can run Gemma 4 E2B inference through the local-llm crate, receiving streaming text output on first use after an automatic model download.
- **SC-002**: Thinking mode produces correctly parsed thinking events for 100% of Gemma 4 outputs that contain thinking delimiters, including outputs where delimiters span streaming chunk boundaries.
- **SC-003**: SmolLM3-3B remains the default local model — no configuration changes required for existing users on any hardware.
- **SC-004**: Developers with capable hardware (16+ GB RAM) can opt into Gemma 4 E2B via preset selection or environment variable, with no Ollama dependency.
- **SC-005**: All Gemma 4 direct inference code is excluded from compilation when the feature flag is disabled, adding zero overhead for users who do not need it.
- **SC-006**: Local Gemma 4 E2B inference provides 128K token context window — a 15x improvement over the previous SmolLM3-3B default (8K tokens).
