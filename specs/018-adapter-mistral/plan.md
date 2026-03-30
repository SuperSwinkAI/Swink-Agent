# Implementation Plan: Adapter — Mistral

**Branch**: `018-adapter-mistral` | **Date**: 2026-03-30 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/018-adapter-mistral/spec.md`

## Summary

Implement the Mistral chat completions adapter with request/response normalization for known API divergences from OpenAI. The adapter holds `AdapterBase` directly (like Azure) and reuses `openai_compat` types for message serialization and SSE parsing while handling Mistral-specific differences: tool call ID format (9-char alphanumeric), `max_tokens` vs `max_completion_tokens`, no `stream_options`, `model_length` finish reason, and message ordering constraints. Includes comprehensive model catalog (12 presets) and full test parity with the OpenAI adapter.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `swink-agent` (core), `reqwest`, `futures`, `serde`/`serde_json`, `tokio`, `tokio-util`, `tracing`, `rand` (ID generation)
**Storage**: N/A
**Testing**: `cargo test`, `wiremock` (mock HTTP server), live test with `MISTRAL_API_KEY`
**Target Platform**: Cross-platform library crate
**Project Type**: Library (adapter module within `swink-agent-adapters` workspace crate)
**Performance Goals**: Streaming latency ≤ OpenAI adapter overhead (normalizer is O(1) per event)
**Constraints**: No additional crate dependencies beyond what's already in workspace
**Scale/Scope**: Single module (~200-300 lines) + test file (~400-500 lines) + catalog entries

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|---|---|---|
| I. Library-First | ✅ Pass | Module in adapters crate, public API via re-export |
| II. Test-Driven Development | ✅ Pass | Full test parity required by spec; live test included |
| III. Efficiency & Performance | ✅ Pass | Normalizer is O(1) per event, ID map is per-invocation |
| IV. Leverage the Ecosystem | ✅ Pass | Reuses openai_compat, classify, sse, finalize |
| V. Provider Agnosticism | ✅ Pass | Implements StreamFn trait, no core changes |
| VI. Safety & Correctness | ✅ Pass | `#[forbid(unsafe_code)]`, error events not panics |

| Constraint | Status | Notes |
|---|---|---|
| Crate count | ✅ Pass | No new crate — module in existing adapters crate |
| MSRV 1.88 | ✅ Pass | No new language features required |
| Concurrency model | ✅ Pass | Single async stream, no spawning |
| Events outward-only | ✅ Pass | Adapter produces events, doesn't consume them |
| No global mutable state | ✅ Pass | ID map is local to stream invocation |

**Post-Phase 1 re-check**: All gates still pass. No new crates, no core changes, no unsafe code.

## Project Structure

### Documentation (this feature)

```text
specs/018-adapter-mistral/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/
│   └── public-api.md    # Phase 1 output
└── tasks.md             # Phase 2 output (via /speckit.tasks)
```

### Source Code (repository root)

```text
adapters/
├── src/
│   ├── mistral.rs          # MistralStreamFn + normalizers (~200-300 lines)
│   ├── lib.rs              # Re-export (existing, update preset keys)
│   ├── remote_presets.rs   # Mistral preset keys + builder (existing, expand)
│   └── openai_compat.rs    # Shared types (no changes)
├── tests/
│   ├── mistral.rs          # Unit tests (~400-500 lines)
│   └── mistral_live.rs     # Live integration test (~80 lines)
src/
└── model_catalog.toml      # Mistral model entries (existing, expand)
```

**Structure Decision**: Module within existing `adapters/` crate. No new crates or structural changes. Follows established pattern from Azure (holds `AdapterBase`, custom `send_request`, reuses `openai_compat` parsing).

## Implementation Phases

### Phase 1: Request Normalization + Basic Streaming

1. Refactor `mistral.rs` from `OpenAiStreamFn` wrapper to `AdapterBase` holder (Azure pattern)
2. Implement `send_request()` with Mistral-specific request construction:
   - `max_tokens` instead of `max_completion_tokens`
   - No `stream_options` in request body
   - `Authorization: Bearer` header
   - URL: `{base_url}/v1/chat/completions`
3. Implement tool call ID generation (9-char alphanumeric via `rand`)
4. Implement `MistralIdMap` for bidirectional ID translation
5. Implement message ordering fix (insert synthetic assistant between tool→user)
6. Wire `StreamFn::stream()` with request normalization → SSE parsing → event stream

### Phase 2: Response Normalization

1. Post-process `parse_oai_sse_stream` output:
   - Remap tool call IDs from Mistral format to harness format via `MistralIdMap`
   - Map `model_length` finish reason to `StopReason::MaxTokens`
   - Map `error` finish reason to error event
2. Handle full tool calls (not incremental) gracefully — openai_compat already handles this case
3. Usage extraction from final chunk (already handled by openai_compat since no `stream_options` is needed)

### Phase 3: Model Catalog + Presets

1. Expand `model_catalog.toml` with all 12 Mistral model presets
2. Expand `remote_presets.rs` with new preset keys (9 new: large, ministral_3b/8b/14b, magistral_medium/small, devstral, pixtral_large, pixtral_12b)
3. Update preset builder match arm (already exists, just verify)

### Phase 4: Tests

1. Unit tests (mock server):
   - Text streaming (happy path)
   - Tool call streaming (single tool)
   - Multi-tool call streaming
   - Error classification (401, 429, 500)
   - Stream cancellation
   - Usage tracking (from final chunk)
   - `model_length` finish reason mapping
   - Tool call ID format verification (9-char in request, harness format in events)
   - Message ordering (tool result → synthetic assistant → user)
   - Request body verification (no `stream_options`, `max_tokens` not `max_completion_tokens`)
2. Live integration test (`#[ignore]`, `MISTRAL_API_KEY`):
   - Text streaming
   - Tool call round-trip

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Mistral API changes tool call ID format | Low | High | 9-char format check in tests; normalizer is isolated |
| Full tool calls break openai_compat parsing | Low | Medium | openai_compat already handles non-incremental deltas |
| Message ordering constraint missed | Medium | High | Explicit test for tool→user sequence |
| Model catalog outdated quickly | High | Low | Users can pass arbitrary model IDs; presets are convenience |

## Dependencies

- **Upstream**: `swink-agent` core (StreamFn trait, types) — stable, no changes needed
- **Shared infra**: `openai_compat`, `classify`, `sse`, `finalize`, `base`, `convert` — all stable
- **External**: Mistral API (v1/chat/completions) — stable, versioned
- **Workspace deps**: `rand` (already present for jitter), all others already in adapters Cargo.toml
