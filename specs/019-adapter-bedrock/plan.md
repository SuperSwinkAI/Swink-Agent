# Implementation Plan: Adapter AWS Bedrock

**Branch**: `019-adapter-bedrock` | **Date**: 2026-04-02 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/019-adapter-bedrock/spec.md`

## Summary

Implement the AWS Bedrock streaming adapter using the ConverseStream API with AWS event-stream binary encoding. The existing stub (`bedrock.rs`) has ~60% reusable code (SigV4 signing, request types, message conversion, crypto helpers). Main work: replace the non-streaming `converse()` with streaming `converse_stream()` using `aws-smithy-eventstream` for binary frame parsing, add `system` field to request body, update the comprehensive model catalog (~50 models across 10+ provider families), and add live tests.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `swink-agent` (core), `swink-agent-adapters` (shared infra), `sha2`/`hmac` (SigV4 signing), `chrono` (timestamps), `aws-smithy-eventstream` (NEW — event-stream frame decoding), `aws-smithy-types` (NEW — event-stream types)
**Storage**: N/A (stateless adapter)
**Testing**: `cargo test` — unit tests (event-stream parsing, SigV4 signing) + live tests (`#[ignore]`, requires AWS credentials)
**Target Platform**: Any (library crate)
**Project Type**: Library
**Performance Goals**: Streaming latency ≤ provider latency (zero buffering overhead)
**Constraints**: No `unsafe`, reuse SigV4 signing from existing stub, new deps only for event-stream parsing
**Scale/Scope**: ~600 lines adapter code (rewrite streaming path), ~200 lines catalog updates, ~200 lines live tests

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | Adapter is a library type in the adapters crate. No new crate needed. |
| II. Test-Driven Development | PASS | Unit tests for event-stream parsing + live tests planned. |
| III. Efficiency & Performance | PASS | True streaming via event-stream — no buffering. SigV4 crypto is one-time per request. |
| IV. Leverage the Ecosystem | PASS | `aws-smithy-eventstream` is AWS's official parser. Constitution says "prefer well-maintained crates over hand-rolled." |
| V. Provider Agnosticism | PASS | `BedrockStreamFn` implements `StreamFn`. Core crate untouched. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]` inherited. CRC validation handled by aws-smithy-eventstream. |

**Architectural Constraints**:
- Crate count: unchanged (adapters crate absorbs this)
- MSRV: 1.88 ✓
- Two new workspace deps: `aws-smithy-eventstream`, `aws-smithy-types` (gated behind `bedrock` feature)

**Post-Phase 1 Re-check**: All gates still pass. New deps are justified by Constitution IV (Leverage the Ecosystem).

## Project Structure

### Documentation (this feature)

```text
specs/019-adapter-bedrock/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── public-api.md
└── tasks.md             # Phase 2 output (created by /speckit.tasks)
```

### Source Code (repository root)

```text
adapters/src/
├── bedrock.rs           # BedrockStreamFn (exists — rewrite streaming path)
├── lib.rs               # Feature-gated mod + pub use (exists)
├── remote_presets.rs     # build_remote_connection match arm for "bedrock" (exists, needs preset updates)
└── classify.rs          # Shared error classifier (unchanged)

src/
└── model_catalog.toml   # Bedrock provider + presets (needs major expansion: 5 → ~50 models)

Cargo.toml               # Add aws-smithy-eventstream, aws-smithy-types to workspace deps
adapters/Cargo.toml      # Gate new deps behind bedrock feature

adapters/tests/
└── bedrock_live.rs      # Live integration tests (new)
```

**Structure Decision**: No new modules or crates. All changes fit within existing adapter crate structure.

## Implementation Phases

### Phase 1: Dependencies & Workspace Setup

- Add `aws-smithy-eventstream` and `aws-smithy-types` to workspace `[workspace.dependencies]`
- Gate them behind `bedrock` feature in `adapters/Cargo.toml`
- Verify `cargo build --features bedrock` compiles

### Phase 2: Streaming Implementation

Replace the non-streaming `converse()` with streaming `converse_stream()`:

1. **Request changes**:
   - Add `system` field to `BedrockRequest` (top-level, not synthetic user message)
   - Change URL path from `/model/{id}/converse` to `/model/{id}/converse-stream`

2. **Event-stream parsing**:
   - Read response body as byte stream (`response.bytes_stream()`)
   - Feed bytes into `MessageFrameDecoder` incrementally
   - For each complete frame: read `:event-type` header, deserialize JSON payload
   - Map to `AssistantMessageEvent` (see research.md R3 for mapping)

3. **Streaming state machine**:
   - Track `current_block_type` (Text/ToolUse) across contentBlockStart/Stop
   - Track `stop_reason` from messageStop
   - Emit `Done` with usage from metadata event

4. **Stop reason mapping**:
   - `end_turn`/`stop_sequence` → `Stop`
   - `tool_use` → `ToolUse`
   - `max_tokens` → `Length`
   - `guardrail_intervened` → `ContentFiltered` error event

5. **Cancellation**: Check `cancellation_token` in the streaming loop via `tokio::select!`

### Phase 3: Model Catalog Update

Expand `src/model_catalog.toml` from 5 to ~50 Bedrock model presets:
- Anthropic (9 models), Meta (10), Amazon (6), Mistral (9), DeepSeek (2), AI21 (3), Cohere (2), OpenAI (2), Qwen (4), Writer (2), Others (4)
- Update `remote_presets.rs` preset key constants to match

### Phase 4: Live Tests

Create `adapters/tests/bedrock_live.rs`:
- Text streaming test (simple prompt → verify incremental deltas)
- Tool call test (prompt with tool definitions → verify tool call events)
- Error handling test (invalid credentials → verify auth error)
- Usage/metadata test (verify token counts in Done event)
- Multi-turn context test (verify conversation continuity)

All tests `#[ignore]`, 30s timeout, use cheapest available model.

### Phase 5: Verification

- `cargo build --workspace` passes
- `cargo test --workspace` passes
- `cargo clippy --workspace -- -D warnings` clean
- `cargo test -p swink-agent-adapters --no-default-features --features bedrock` compiles and runs in isolation
- Live tests pass with real AWS credentials (manual verification)

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| Two new workspace deps (`aws-smithy-eventstream`, `aws-smithy-types`) | Binary event-stream protocol requires specialized parsing with CRC validation | Hand-rolling is error-prone and violates Constitution IV |

## Risks & Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| `aws-smithy-eventstream` API changes | Low | Pin to compatible version range in workspace deps |
| Event-stream frame format differences across Bedrock models | Low | ConverseStream API is model-agnostic (unified format) |
| Model IDs stale or region-specific | Medium | Catalog is data, easily updated; IDs use cross-region inference profile format |
| SigV4 signing incompatible with streaming endpoint | Very Low | Same signing process applies to all Bedrock endpoints |
