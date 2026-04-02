# Implementation Plan: Adapter xAI

**Branch**: `017-adapter-xai` | **Date**: 2026-04-02 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/017-adapter-xai/spec.md`

## Summary

Implement the xAI (Grok) streaming chat adapter. The adapter code (`xai.rs`) already exists as a thin wrapper around `OpenAiStreamFn` — this is architecturally correct since xAI follows the OpenAI chat completions protocol exactly. Remaining work: update stale model catalog presets from grok-3 to current grok-4.x models, add comprehensive live tests, and wire up the `remote_presets` module for catalog-driven construction.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `swink-agent` (core), `swink-agent-adapters` (shared infra: `openai_compat`, `classify`, `sse`, `convert`, `base`)
**Storage**: N/A (stateless adapter)
**Testing**: `cargo test` — unit tests (mocked SSE) + live tests (`#[ignore]`, requires `XAI_API_KEY`)
**Target Platform**: Any (library crate)
**Project Type**: Library
**Performance Goals**: Streaming latency ≤ provider latency (zero buffering overhead)
**Constraints**: No `unsafe`, no new dependencies, reuse shared infra exclusively
**Scale/Scope**: ~50 lines adapter code (already written), ~100 lines catalog updates, ~200 lines live tests

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | Adapter is a library type in the adapters crate. No new crate needed. |
| II. Test-Driven Development | PASS | Live tests planned before any behavioral changes. |
| III. Efficiency & Performance | PASS | Zero-copy delegation to `OpenAiStreamFn`. No extra allocations. |
| IV. Leverage the Ecosystem | PASS | Reuses all shared infra (`openai_compat`, `sse`, `classify`, `convert`). No hand-rolled protocol handling. |
| V. Provider Agnosticism | PASS | `XAiStreamFn` implements `StreamFn`. Core crate untouched. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]` inherited from crate root. Compile-time `Send + Sync` assertion present. |

**Architectural Constraints**:
- Crate count: unchanged (adapters crate absorbs this)
- MSRV: 1.88 ✓
- No new dependencies required

**Post-Phase 1 Re-check**: All gates still pass. No design decisions required normalization or new crates.

## Project Structure

### Documentation (this feature)

```text
specs/017-adapter-xai/
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
├── xai.rs               # XAiStreamFn (exists — ~50 lines, delegates to OpenAiStreamFn)
├── lib.rs               # Feature-gated mod + pub use (exists)
├── remote_presets.rs     # build_remote_connection match arm for "xai" (needs update)
└── openai_compat.rs     # Shared types/parser (unchanged)

src/
└── model_catalog.toml   # xAI provider + presets (needs update: grok-3 → grok-4.x)

adapters/tests/
└── xai_live.rs          # Live integration tests (new)
```

**Structure Decision**: No new modules or crates. All changes fit within existing adapter crate structure following the established pattern from OpenAI, Mistral, and Gemini adapters.

## Implementation Phases

### Phase 1: Model Catalog Update

Update `src/model_catalog.toml`:
- Replace 2 stale `grok-3`/`grok-3-fast` presets with 5 current grok-4.x models
- Update context windows from 131K to 2M tokens
- Update pricing to current rates
- Add `"structured_output"` capability to all presets

### Phase 2: Remote Presets Wiring

Update `adapters/src/remote_presets.rs`:
- Verify the `"xai"` match arm in `build_remote_connection()` creates `XAiStreamFn`
- Ensure it correctly resolves base URL and API key from preset/env

### Phase 3: Live Tests

Create `adapters/tests/xai_live.rs` following the OpenAI/Mistral live test pattern:
- Text streaming test (simple prompt → verify incremental deltas + terminal event)
- Tool call test (prompt with tool definitions → verify tool call events with valid JSON args)
- Error handling test (invalid API key → verify auth error classification)
- Cancellation test (cancel mid-stream → verify clean termination)
- Multi-tool test (prompt likely to trigger multiple tool calls → verify separate indexed blocks)

All tests `#[ignore]` (require `XAI_API_KEY`), 30s timeout, use cheapest model (`grok-4-1-fast-non-reasoning`).

### Phase 4: Verification

- `cargo build --workspace` passes
- `cargo test --workspace` passes
- `cargo clippy --workspace -- -D warnings` clean
- `cargo test -p swink-agent-adapters --no-default-features --features xai` compiles and runs
- Live tests pass with real xAI API key (manual verification)

## Complexity Tracking

No constitution violations. No complexity justifications needed.

## Risks & Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| xAI rejects `stream_options` field | Low | Lenient parsing handles gracefully; can add custom request type as follow-up |
| Stale model IDs (grok-4.x renamed/deprecated) | Low | Catalog is data, easily updated without code changes |
| Usage data format differs from OpenAI | Low | Shared parser already handles usage in any chunk position |
