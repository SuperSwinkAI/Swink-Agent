# Gemma 4 E2B Integration Plan

## Overview

Add full Gemma 4 model family support to `swink-agent`, with Gemma 4 E2B as the new default local/edge model fallback. This replaces SmolLM3-3B (8K context, no thinking, no tools) with a significantly more capable local model (128K context, built-in reasoning, standard roles).

**Key facts from research:**
- Gemma 4 E2B: 2.3B effective params (5.1B total with Per-Layer Embeddings), 128K context, Apache 2.0, **not gated**
- GGUF quantizations widely available: bartowski (~3.46 GB Q4_K_M), unsloth (Dynamic 2.0), ggml-org
- Ollama `gemma4:e2b` tag **already exists** (requires Ollama v0.20.0+)
- Thinking output uses `<|channel>thought...<channel|>` format (NOT `<think>...</think>`)
- Ollama handles thinking natively via `think: true` API field — no manual `<|think|>` injection needed
- **mistral.rs 0.7 does NOT support Gemma 4** — v0.8 adds it but uses `MultimodalModelBuilder` (not `GgufModelBuilder`) and has an open NaN logits bug (#2051)

---

## Spec Strategy

**New spec needed:** `041-gemma4-local-default`
**Existing specs to update:** 008, 014, 022

A single new spec (041) captures the Gemma 4-specific integration work. The three existing specs get lightweight addenda for their respective domains.

---

## Critical Risk: mistral.rs Incompatibility (Local-LLM Path)

The current `swink-agent-local-llm` crate uses `mistralrs = "0.7"` with `GgufModelBuilder`. Gemma 4 support requires:

1. **Upgrade to `mistralrs = "0.8"`** — breaking version bump with API changes
2. **Switch to `MultimodalModelBuilder`** for Gemma 4 — `GgufModelBuilder` does not support Gemma architecture (any version)
3. **Open bug**: mistral.rs #2051 — NaN logits and infinite hangs on complex prompts with Gemma 4

**Recommendation:** Prioritize the **Ollama path** (Phase 1-2) which works today with zero code changes to the adapter. Defer the local-llm direct-inference path (Phase 3) until mistral.rs stabilizes Gemma 4 support. Users wanting local inference without Ollama can use llama.cpp server or vLLM (both expose OpenAI-compatible APIs that work with our existing `openai_compat` adapter, zero code changes).

---

## Phase 1: Model Catalog & Ollama Presets (Low Risk, Immediate)

### 1.1 Add Gemma 4 entries to `src/model_catalog.toml`

Add Ollama preset under the existing Ollama-compatible provider configuration:

```toml
[[providers.presets]]
id = "gemma4_e2b"
display_name = "Gemma 4 E2B (Ollama)"
group = "default"
model_id = "gemma4:e2b"
capabilities = ["text", "thinking", "tools", "streaming"]
context_window_tokens = 128000
max_output_tokens = 8192
```

Add larger variants for users with more resources:

```toml
[[providers.presets]]
id = "gemma4_e4b"
display_name = "Gemma 4 E4B (Ollama)"
group = "default"
model_id = "gemma4:e4b"
capabilities = ["text", "thinking", "tools", "streaming"]
context_window_tokens = 128000
max_output_tokens = 8192

[[providers.presets]]
id = "gemma4_26b"
display_name = "Gemma 4 26B MoE (Ollama)"
group = "large"
model_id = "gemma4:26b"
capabilities = ["text", "thinking", "tools", "streaming"]
context_window_tokens = 256000
max_output_tokens = 8192
```

Demote the existing `smollm3_3b` preset: set `include_by_default = false`, `group = "legacy"`.

**Spec update:** 008-model-catalog-presets — addendum for new presets and default group change.

### 1.2 Update `ModelCapabilities` for Gemma 4

When building a `ModelSpec` for Gemma 4 E2B (via catalog preset resolution):

```rust
ModelCapabilities {
    supports_thinking: true,
    supports_vision: false,        // E2B has vision encoder but text-only in chat mode
    supports_tool_use: true,       // via Ollama native tool calling
    supports_streaming: true,
    supports_structured_output: false,  // known Ollama bug #15260 with think+format
    max_context_window: 128_000,
    max_output_tokens: 8_192,
}
```

### 1.3 Context management

The existing sliding-window context manager (`src/context.rs`) works with any `max_context_window` value. 128K is well within the range already handled (Anthropic = 200K, Gemini = 1M). **No changes needed.**

---

## Phase 2: Ollama Adapter Thinking Support (Medium Risk)

### 2.1 Enable `think` field in Ollama requests

**File:** `adapters/src/ollama.rs`

The `OllamaChatRequest` struct (line ~37) already has a `think: Option<bool>` field. Currently it's always set to `None` (line ~243).

**Change:** When the model's `ModelSpec` has `capabilities.supports_thinking == true` or `thinking_level != Off`, set `think: Some(true)` in the request.

```rust
// In send_request(), around line 240
let think = if model.capabilities().supports_thinking {
    Some(true)
} else {
    None
};
```

**Important: Do NOT manually inject `<|think|>` into the system prompt.** Ollama handles the chat template internally — setting `think: true` causes Ollama to inject the `<|think|>` control token via the model's Jinja template. Manual injection would double-apply the token.

### 2.2 Verify thinking response parsing

The Ollama adapter already parses the `thinking` field from `OllamaResponseMessage` (lines ~390-403) and emits `ThinkingStart`/`ThinkingDelta`/`ThinkingEnd` events. Ollama separates thinking content into its own response field, so the adapter never sees raw `<|channel>thought...<channel|>` tokens.

**No response parsing changes needed for the Ollama path.**

### 2.3 Multi-turn thinking content stripping

Gemma 4 requires that thinking content from previous turns be stripped before sending the next request. The Ollama adapter's `OllamaConverter::assistant_message()` (lines ~282-312) iterates `ContentBlock` variants — `ContentBlock::Thinking` falls through to the `_ => {}` catch-all and is silently dropped.

**Already correct.** Thinking blocks from previous turns are not sent back to Ollama.

### 2.4 Known Ollama bug: `think: false` + structured output

Ollama issue #15260: setting `think: false` with the `format` parameter breaks structured output (format constraints silently ignored). This means we **cannot** combine `supports_structured_output: true` with Gemma 4 in Ollama.

**Mitigation:** Set `supports_structured_output: false` in Gemma 4 capabilities (already done in 1.2).

### 2.5 Role mapping verification

Gemma 4 uses standard `system`/`user`/`assistant` roles. The Ollama adapter already maps to these exact roles:

| swink-agent type | Ollama role | Gemma 4 expected |
|---|---|---|
| System prompt | `"system"` | `system` |
| UserMessage | `"user"` | `user` |
| AssistantMessage | `"assistant"` | `assistant` |
| ToolResultMessage | `"tool"` | `tool` |

**No role mapping changes needed.**

### 2.6 Tool call format

Gemma 4's native tool call format uses custom serialization (`<|tool_call>call:fn{...}<tool_call|>`), but Ollama abstracts this — the adapter sees standard JSON tool calls via Ollama's `tool_calls` response field. The existing `OllamaTool`/`OllamaToolCall` serialization is compatible.

**No changes needed.**

**Spec update:** 014-adapter-ollama — addendum for `think` field usage with Gemma 4.

---

## Phase 3: Local-LLM Direct Inference (High Risk, Deferred)

### 3.1 mistral.rs upgrade: 0.7 → 0.8

**File:** `local-llm/Cargo.toml` (line 27)

This is a **major version bump** with breaking API changes:
- `GgufModelBuilder` does NOT support Gemma 4 architecture (uses Per-Layer Embeddings)
- Gemma 4 requires `MultimodalModelBuilder` even for text-only use
- The builder API may have changed between 0.7 and 0.8

**Required changes in `local-llm/src/model.rs`:**
- Add model-family detection: inspect GGUF metadata or config to determine if the model is Gemma 4
- Branch builder logic: `GgufModelBuilder` for existing models, `MultimodalModelBuilder` for Gemma 4
- This breaks the current "fully model-agnostic" design (currently zero family-specific branching)

### 3.2 Add `ModelPreset::Gemma4_E2B` variant

**File:** `local-llm/src/preset.rs`

```rust
pub enum ModelPreset {
    Gemma4_E2B,          // NEW — default chat model
    SmolLM3_3B,          // demoted to legacy
    EmbeddingGemma300M,
}
```

Config for the new variant:

```rust
ModelPreset::Gemma4_E2B => ModelConfig {
    repo_id: env::var("LOCAL_MODEL_REPO")
        .unwrap_or_else(|_| "bartowski/google_gemma-4-E2B-it-GGUF".to_string()),
    filename: env::var("LOCAL_MODEL_FILE")
        .unwrap_or_else(|_| "gemma-4-E2B-it-Q4_K_M.gguf".to_string()),
    gpu_layers: env::var("LOCAL_GPU_LAYERS")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(0),
    context_length: env::var("LOCAL_CONTEXT_LENGTH")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(131072),
    chat_template: None,
},
```

Update `DEFAULT_LOCAL_PRESET_ID` from `"smollm3_3b"` to `"gemma4_e2b"`.

### 3.3 Thinking output parsing for direct inference

**Critical finding:** The current `stream.rs` `extract_thinking_delta()` (line 405) parses `<think>...</think>` tags. Gemma 4 uses `<|channel>thought\n...<channel|>` format instead.

**Required change in `local-llm/src/stream.rs`:**
- Add a second parser for `<|channel>thought` / `<channel|>` delimiters
- The parser needs to handle cross-delta boundaries (current `<think>` parser only works within a single delta)
- Model-family detection needed to choose the correct parser, OR make the parser try both formats

### 3.4 System prompt `<|think|>` injection for direct inference

When using mistral.rs directly (not Ollama), the `<|think|>` token must be injected into the system prompt manually because there's no higher-level `think: true` API.

**File:** `local-llm/src/convert.rs`

Add model-aware system prompt preparation:

```rust
fn prepare_system_prompt(config: &ModelConfig, system_prompt: &str) -> String {
    if config.is_gemma4() && thinking_enabled {
        format!("<|think|>\n{}", system_prompt)
    } else {
        system_prompt.to_string()
    }
}
```

Add `is_gemma4()` to `ModelConfig`:

```rust
impl ModelConfig {
    pub fn is_gemma4(&self) -> bool {
        self.repo_id.contains("gemma-4") || self.repo_id.contains("gemma4")
    }
}
```

### 3.5 Blocking issues for Phase 3

| Issue | Status | Impact |
|---|---|---|
| mistral.rs 0.8 API changes | Unknown scope | May require significant refactoring of model.rs |
| NaN logits bug (#2051) | Open, no fix ETA | Gemma 4 may produce garbage on complex prompts |
| `MultimodalModelBuilder` required | Confirmed | Cannot use existing `GgufModelBuilder` code path |
| `<|channel>thought` parser | Not implemented | New parser needed in stream.rs |

**Recommendation:** Gate Phase 3 behind a `gemma4` feature flag. Ship Phases 1-2 first (Ollama path works today). Revisit when mistral.rs 0.8 stabilizes.

**Spec update:** 022-local-llm-crate — addendum for deferred Gemma 4 direct inference, mistral.rs version requirements.

---

## Phase 4: Zero-Code-Change Alternative Backends

These backends work **today** with the existing `openai_compat` adapter — no code changes needed, just documentation.

### 4.1 llama.cpp server

```bash
# Download GGUF
huggingface-cli download bartowski/google_gemma-4-E2B-it-GGUF \
    gemma-4-E2B-it-Q4_K_M.gguf --local-dir ./models

# Start server with thinking + tool calling
llama-server -m ./models/gemma-4-E2B-it-Q4_K_M.gguf \
    --chat-template-kwargs '{"enable_thinking":true}' \
    --port 8080

# Use with swink-agent's OpenAI adapter
OPENAI_API_KEY=dummy OPENAI_BASE_URL=http://localhost:8080/v1 \
    cargo run -p swink-agent-tui
```

Day-0 Gemma 4 support. Streaming, tool calling, and thinking all work. ~10 tok/s on E2B with CPU, faster with Metal/CUDA.

### 4.2 vLLM

```bash
vllm serve google/gemma-4-E2B-it \
    --max-model-len 16384 \
    --enable-auto-tool-choice \
    --reasoning-parser gemma4 \
    --tool-call-parser gemma4

# Use with swink-agent's OpenAI adapter
OPENAI_API_KEY=dummy OPENAI_BASE_URL=http://localhost:8000/v1 \
    cargo run -p swink-agent-tui
```

Highest throughput option. Dedicated Gemma 4 tool/thinking parsers built in. Thinking content appears in `reasoning_content` field.

**Note:** vLLM's `reasoning_content` field maps to the OpenAI extended format. The `openai_compat` adapter may need a small update to parse this field and emit `ThinkingStart`/`ThinkingDelta`/`ThinkingEnd` events. Worth investigating.

### 4.3 LM Studio

GUI app, one-click download of Gemma 4 E2B. Exposes OpenAI-compatible server.

**Caveat:** Known bug (lmstudio-bug-tracker#1066) where tool calling fails when `stream: true`. May require `stream: false` workaround until patched.

### 4.4 Custom Ollama Modelfile (for fine-tuned variants)

```dockerfile
FROM ./gemma-4-E2B-it-Q4_K_M.gguf
PARAMETER temperature 0.7
PARAMETER num_ctx 131072
```

```bash
ollama create my-gemma4 -f Modelfile
ollama run my-gemma4
```

Useful for running community quantizations or fine-tuned models not in the Ollama library.

### 4.5 Documentation deliverable

Add a `docs/local-models.md` or section in the TUI README showing how to use Gemma 4 E2B with each backend. Emphasize that Ollama is the primary supported path, but llama.cpp/vLLM/LM Studio work via the OpenAI adapter with zero code changes.

---

## Phase 5: Testing

### 5.1 Unit tests

**File:** `adapters/tests/ollama.rs` (new tests)
- `gemma4_think_field_enabled` — verify `think: Some(true)` when model supports thinking
- `non_thinking_model_think_field_absent` — verify `think: None` for models without thinking
- `gemma4_thinking_response_parsed` — verify thinking field in response emits correct events

**File:** `src/model_catalog.rs` (existing test module)
- Verify `gemma4_e2b` preset loads with correct context window (128000)
- Verify `gemma4_e2b` has `thinking` capability
- Verify old `smollm3_3b` is no longer `include_by_default`

**File (Phase 3 only):** `local-llm/src/preset.rs`
- `gemma4_e2b_default_config` — verify repo_id, filename, context_length defaults
- `gemma4_e2b_env_override` — verify env var overrides

**File (Phase 3 only):** `local-llm/src/stream.rs`
- `gemma4_channel_thought_parsing` — verify `<|channel>thought...<channel|>` extraction
- `gemma4_channel_thought_cross_delta` — verify parsing across chunk boundaries

### 5.2 Integration tests (ignored by default)

**File:** `adapters/tests/ollama_live.rs` (new ignored tests)
- `live_gemma4_text_stream` — stream text from `gemma4:e2b` via local Ollama
- `live_gemma4_thinking_stream` — verify thinking blocks arrive with `think: true`
- `live_gemma4_tool_call` — verify tool calling works end-to-end
- `live_gemma4_multi_turn` — verify thinking blocks stripped between turns

**File (Phase 3 only):** `local-llm/tests/local_live.rs`
- `live_gemma4_e2b_inference` — download and run inference
- `live_gemma4_e2b_thinking` — verify channel-based thinking parsing

---

## Phase 6: Specs & Documentation

### 6.1 New spec: `specs/041-gemma4-local-default/`

Create full spec with:
- `spec.md` — User stories: run Gemma 4 via Ollama, thinking mode, fallback to OpenAI-compatible servers, future direct inference
- `plan.md` — This document, condensed
- `tasks.md` — Task breakdown matching the phases above
- `data-model.md` — Catalog presets, `think` field usage, `<|channel>thought` format

### 6.2 Spec addenda

| Spec | Update |
|---|---|
| `008-model-catalog-presets` | Add Gemma 4 E2B/E4B/26B presets, note default group change |
| `014-adapter-ollama` | Document `think: true` field for thinking-capable models |
| `022-local-llm-crate` | Document deferred Gemma 4 direct inference, mistral.rs 0.8 requirement, `<\|channel>thought` parser |

### 6.3 CLAUDE.md updates

- Add Gemma 4 to "Active Technologies" under local-llm and adapters
- Document the Ollama-first strategy and alternative backend options
- Note mistral.rs 0.8 requirement for direct inference

---

## File Change Summary

### Phase 1-2 (Ship immediately)

| File | Change | Description |
|---|---|---|
| `src/model_catalog.toml` | Edit | Add `gemma4_e2b`, `gemma4_e4b`, `gemma4_26b` presets; demote `smollm3_3b` |
| `adapters/src/ollama.rs` | Edit | Set `think: Some(true)` when model supports thinking (~5 lines) |
| `adapters/tests/ollama.rs` | Edit | Add thinking field unit tests |
| `adapters/tests/ollama_live.rs` | Edit | Add Gemma 4 live integration tests |
| `src/model_catalog.rs` | Edit | Update catalog tests for new presets |
| `specs/041-gemma4-local-default/` | New | Full spec |

### Phase 3 (Deferred, behind feature gate)

| File | Change | Description |
|---|---|---|
| `local-llm/Cargo.toml` | Edit | Bump `mistralrs` to `"0.8"` |
| `local-llm/src/preset.rs` | Edit | Add `Gemma4_E2B` variant; change default |
| `local-llm/src/model.rs` | Edit | Add `MultimodalModelBuilder` path; add `is_gemma4()` |
| `local-llm/src/stream.rs` | Edit | Add `<\|channel>thought...<channel\|>` parser |
| `local-llm/src/convert.rs` | Edit | Add `<\|think\|>` system prompt injection for direct inference |
| `local-llm/tests/local_live.rs` | Edit | Add Gemma 4 live tests |

---

## Risks & Mitigations (Updated with Research)

| Risk | Severity | Status | Mitigation |
|---|---|---|---|
| mistral.rs 0.7 doesn't support Gemma 4 | **High** | **Confirmed** | Defer local-llm path; use Ollama/llama.cpp/vLLM instead |
| mistral.rs 0.8 NaN logits bug (#2051) | **High** | **Open** | Gate behind feature flag; wait for upstream fix |
| mistral.rs 0.8 requires `MultimodalModelBuilder` | **Medium** | **Confirmed** | Adds model-family branching to currently family-agnostic code |
| Ollama `gemma4:e2b` tag unavailable | ~~Medium~~ | **Resolved** | Tag exists since Ollama v0.20.0 |
| GGUF quantization unavailable | ~~Medium~~ | **Resolved** | bartowski, unsloth, ggml-org all provide GGUF |
| `<think>` parser incompatible with Gemma 4 | **Medium** | **Confirmed** | Gemma 4 uses `<\|channel>thought`, not `<think>`; new parser needed (Phase 3 only) |
| Ollama `think:false` + structured output bug (#15260) | **Low** | **Open** | Set `supports_structured_output: false` for Gemma 4 |
| LM Studio streaming + tool calling bug (#1066) | **Low** | **Open** | Document workaround; LM Studio is optional backend |
| vLLM `reasoning_content` field not parsed by openai_compat | **Low** | **Unverified** | Check if adapter handles extended response fields |
| Gemma 4 E2B download ~3.5 GB for CI | **Low** | N/A | Keep live tests `#[ignore]`; unit tests use mocks |

---

## Implementation Order

### Immediate (Phase 1-2): Ollama-first path
1. Add catalog presets to `model_catalog.toml` — zero-risk data change
2. Set `think: Some(true)` in Ollama adapter for thinking-capable models — ~5 lines
3. Unit tests for thinking field and catalog entries
4. Live integration tests (ignored) for Gemma 4 via Ollama
5. Write spec 041 and spec addenda
6. Documentation for alternative backends (llama.cpp, vLLM, LM Studio)

**Scope:** ~50 lines Rust, ~150 lines tests, spec documents

### Deferred (Phase 3): Direct local inference
1. Bump mistral.rs to 0.8 (when NaN bug is fixed)
2. Add `MultimodalModelBuilder` code path in model.rs
3. Add `<|channel>thought` parser in stream.rs
4. Add `<|think|>` injection in convert.rs
5. Add `Gemma4_E2B` preset and make it default
6. Gate behind `gemma4` feature flag until stable

**Scope:** ~200 lines Rust, ~100 lines tests, depends on upstream fix

---

## Alternative Backend Comparison

For users who want Gemma 4 E2B locally today:

| Backend | Protocol | swink-agent Adapter | Streaming | Tools | Thinking | Code Changes |
|---|---|---|---|---|---|---|
| **Ollama** (recommended) | NDJSON | Ollama adapter | Yes | Yes | Yes (`think: true`) | Phase 2 only |
| **llama.cpp server** | SSE | OpenAI (`openai_compat`) | Yes | Yes | Yes (template flag) | **None** |
| **vLLM** | SSE | OpenAI (`openai_compat`) | Yes | Yes | Yes (`--reasoning-parser`) | **None** |
| **LM Studio** | SSE | OpenAI (`openai_compat`) | Yes | Buggy (#1066) | Yes | **None** |
| **mistral.rs direct** | In-process | `swink-agent-local-llm` | Yes | TBD | TBD | Phase 3 (deferred) |
| **LiteRT-LM** | Unknown | Possibly new adapter | Yes | Via presets | Yes | **Investigation needed** |

---

## Checklist

### Phase 1: Model Catalog & Ollama Presets
- [x] Add `gemma4_e2b` preset to `src/model_catalog.toml` (model_id: `gemma4:e2b`, 128K context, capabilities: text/thinking/tools/streaming)
- [x] Add `gemma4_e4b` preset to `src/model_catalog.toml` (model_id: `gemma4:e4b`, 128K context)
- [x] Add `gemma4_26b` preset to `src/model_catalog.toml` (model_id: `gemma4:26b`, 256K context, group: `large`)
- [x] Demote `smollm3_3b` preset: `include_by_default = false`, `group = "legacy"`
- [x] Verify `ModelCapabilities` resolution sets `supports_thinking: true`, `supports_structured_output: false` for Gemma 4 presets
- [x] Update catalog unit tests: assert `gemma4_e2b` is default, correct 128K context window, `smollm3_3b` demoted (`src/model_catalog.rs`)
- [x] Update spec 008-model-catalog-presets addendum

### Phase 2: Ollama Adapter Thinking Support
- [x] Wire `think` field in `OllamaChatRequest`: set `Some(true)` when `model.thinking_level != ThinkingLevel::Off` (`adapters/src/ollama.rs` line 245)
- [x] Verify existing thinking response parsing (`OllamaResponseMessage.thinking` field) works — no changes needed
- [x] Verify multi-turn thinking stripping: `ContentBlock::Thinking` dropped in `OllamaConverter::assistant_message()` — already correct
- [x] **Did NOT inject `<|think|>` into system prompt** — Ollama handles this via chat template
- [x] Add unit test: `ollama_think_field_set_when_thinking_enabled` — request includes `"think":true` for thinking-capable model (wiremock `body_string_contains` matcher)
- [x] Add unit test: `ollama_think_field_absent_when_thinking_off` — request omits `think` for non-thinking model
- [x] Existing unit test `ollama_thinking_stream` already covers thinking response parsing — no new test needed
- [x] Add live test (ignored): `live_gemma4_e2b_thinking` — thinking blocks arrive with `think: true` via `gemma4:e2b`
- [ ] Add live test (ignored): `live_gemma4_text_stream` — text streaming via `gemma4:e2b` (future)
- [ ] Add live test (ignored): `live_gemma4_tool_call` — tool calling end-to-end (future)
- [ ] Add live test (ignored): `live_gemma4_multi_turn` — thinking stripped between turns (future)
- [x] Update spec 014-adapter-ollama addendum
- [x] Update spec 022-local-llm-crate addendum (Gemma 4 deferred for local-llm)
- [x] All 18 Ollama wiremock tests pass
- [x] All 20 model catalog tests pass
- [x] No new clippy warnings (2 pre-existing `AdapterBase` dead-code warnings unchanged)

### Phase 3: Local-LLM Direct Inference (Deferred)
- [ ] **Prerequisite:** Monitor mistral.rs #2051 (NaN logits bug) — do not start until fixed
- [ ] Bump `mistralrs` from `"0.7"` to `"0.8"` in `local-llm/Cargo.toml`
- [ ] Audit mistral.rs 0.7 → 0.8 API breaking changes; fix compilation errors in `model.rs`, `stream.rs`, `embedding.rs`
- [ ] Add model-family detection: `ModelConfig::is_gemma4()` (check repo_id for `"gemma-4"` / `"gemma4"`)
- [ ] Add `MultimodalModelBuilder` code path in `model.rs` for Gemma 4 (alongside existing `GgufModelBuilder`)
- [ ] Add `ModelPreset::Gemma4_E2B` variant in `preset.rs` (repo: `bartowski/google_gemma-4-E2B-it-GGUF`, file: `gemma-4-E2B-it-Q4_K_M.gguf`, context: 131072)
- [ ] Update `DEFAULT_LOCAL_PRESET_ID` from `"smollm3_3b"` to `"gemma4_e2b"`
- [ ] Add `<|channel>thought...<channel|>` parser in `stream.rs` (cross-delta boundary support required)
- [ ] Add `<|think|>` system prompt injection in `convert.rs` for direct inference path
- [ ] Gate all Phase 3 changes behind `gemma4` feature flag
- [ ] Add unit test: `gemma4_e2b_default_config` — preset defaults correct
- [ ] Add unit test: `gemma4_e2b_env_override` — env vars override preset
- [ ] Add unit test: `is_gemma4_detection` — family detection for various repo IDs
- [ ] Add unit test: `gemma4_channel_thought_parsing` — `<|channel>thought` extraction
- [ ] Add unit test: `gemma4_channel_thought_cross_delta` — parsing across chunk boundaries
- [ ] Add live test (ignored): `live_gemma4_e2b_inference` — download + inference
- [ ] Add live test (ignored): `live_gemma4_e2b_thinking` — channel-based thinking parsing
- [ ] Update spec 022-local-llm-crate addendum

### Phase 4: Documentation & Alternative Backends
- [ ] Write `docs/local-models.md` with Gemma 4 setup instructions for each backend
- [ ] Document llama.cpp server usage (download GGUF, start server, point OpenAI adapter)
- [ ] Document vLLM usage (serve command, reasoning/tool parsers, OpenAI adapter)
- [ ] Document LM Studio usage (GUI download, note streaming+tools bug #1066)
- [ ] Document custom Ollama Modelfile for fine-tuned variants
- [ ] Investigate: does `openai_compat` adapter parse vLLM's `reasoning_content` field for thinking events?

### Phase 5: Specs & Project Hygiene
- [ ] Create `specs/041-gemma4-local-default/spec.md`
- [ ] Create `specs/041-gemma4-local-default/plan.md`
- [ ] Create `specs/041-gemma4-local-default/tasks.md`
- [ ] Create `specs/041-gemma4-local-default/data-model.md`
- [ ] Update `CLAUDE.md` — Active Technologies, Lessons Learned
- [ ] Update `local-llm/CLAUDE.md` — default model change, thinking behavior
- [ ] Run `cargo test --workspace --features testkit` — all existing tests still pass
- [ ] Run `cargo clippy --workspace -- -D warnings` — zero new warnings
