# Implementation Plan: Artifact Service

**Branch**: `036-artifact-service` | **Date**: 2026-04-02 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/036-artifact-service/spec.md`

## Summary

Session-attached versioned artifact storage for agent-produced outputs. Introduces an `ArtifactStore` trait in the core crate (feature-gated) with save/load/list/delete operations scoped by session ID. A new `swink-agent-artifacts` workspace crate provides `FileArtifactStore` and `InMemoryArtifactStore` implementations. Built-in LLM tools (save, load, list) are gated under a separate `artifact-tools` feature. An `AgentEvent::ArtifactSaved` variant integrates with the existing event system.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `serde`, `serde_json`, `tokio` (fs + sync), `chrono` (timestamps), `tracing` (diagnostics), `thiserror` (errors), `futures` (streaming trait), `schemars` (tool schemas) — all workspace deps
**Storage**: Local filesystem (versioned files + JSON metadata sidecar); in-memory (`HashMap`) for testing
**Testing**: `cargo test --workspace` — unit tests in each module, integration tests in `tests/`
**Target Platform**: Cross-platform (Linux, macOS, Windows)
**Project Type**: Library crate (workspace member)
**Performance Goals**: Streaming I/O for 10MB+ artifacts without proportional heap allocation
**Constraints**: No `unsafe`, `#[forbid(unsafe_code)]` at crate root. `Send + Sync` on all traits. Zero overhead when feature disabled.
**Scale/Scope**: Per-session artifact storage; no cross-session versioning; no size limits enforced by framework

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | ✅ PASS | New `swink-agent-artifacts` crate is independently compilable and testable. Core trait in `swink-agent` behind feature gate. |
| II. Test-Driven Development | ✅ PASS | Plan requires tests before implementation. `InMemoryArtifactStore` enables fast unit tests. |
| III. Efficiency & Performance | ✅ PASS | `StreamingArtifactStore` extension trait for large artifacts. Base API uses `Vec<u8>` — simple path first. |
| IV. Leverage the Ecosystem | ✅ PASS | Uses `tokio::fs` for async I/O, `serde_json` for metadata, `chrono` for timestamps — all existing workspace deps. |
| V. Provider Agnosticism | ✅ PASS | Artifact storage is provider-independent; trait abstraction allows any backend. |
| VI. Safety & Correctness | ✅ PASS | `#[forbid(unsafe_code)]`, concurrent access via `tokio::sync::Mutex`, event-based observability. |

**Crate count**: Constitution specifies 7 workspace members. Current workspace has 10 (core, adapters, auth, eval, local-llm, macros, memory, policies, tui, xtask). Adding `artifacts` brings it to 11. **Justified**: Artifacts involve large binary I/O with different performance characteristics than the JSONL-based memory crate. Merging into memory would violate single-responsibility and couple binary storage decisions to conversation persistence.

## Project Structure

### Documentation (this feature)

```text
specs/036-artifact-service/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
│   └── public-api.md    # ArtifactStore trait contract
└── tasks.md             # Phase 2 output (/speckit.tasks)
```

### Source Code (repository root)

```text
artifacts/
├── Cargo.toml           # swink-agent-artifacts crate
├── src/
│   ├── lib.rs           # Re-exports, #[forbid(unsafe_code)]
│   ├── error.rs         # ArtifactError enum
│   ├── fs_store.rs      # FileArtifactStore implementation
│   ├── memory_store.rs  # InMemoryArtifactStore implementation
│   ├── streaming.rs     # StreamingArtifactStore trait + FileArtifactStore impl
│   └── validate.rs      # Artifact name validation
└── tests/
    ├── fs_store.rs       # FileArtifactStore integration tests
    ├── memory_store.rs   # InMemoryArtifactStore unit tests
    └── streaming.rs      # StreamingArtifactStore tests

# Core crate additions (behind `artifact-store` feature gate)
src/
├── artifact.rs          # ArtifactStore trait, ArtifactData, ArtifactVersion, ArtifactMeta types
└── loop_/event.rs       # + AgentEvent::ArtifactSaved variant

# Core crate additions (behind `artifact-tools` feature gate)
src/tools/
├── save_artifact.rs     # SaveArtifactTool
├── load_artifact.rs     # LoadArtifactTool
└── list_artifacts.rs    # ListArtifactsTool
```

**Structure Decision**: New `artifacts/` workspace member for implementations. Core trait and types live in `src/artifact.rs` behind `artifact-store` feature. Built-in tools in `src/tools/` behind `artifact-tools` feature. This follows the same pattern as `builtin-tools` gating `BashTool`/`ReadFileTool`/`WriteFileTool`.

## Implementation Phases

### Phase 1: Core Trait & Types (P1 — FR-001 through FR-009, FR-014, FR-017)

Define the `ArtifactStore` trait and supporting types in `src/artifact.rs`:

- `ArtifactData` struct: `content: Vec<u8>`, `content_type: String`, `metadata: HashMap<String, String>`
- `ArtifactVersion` struct: `name: String`, `version: u32`, `created_at: DateTime<Utc>`, `size: usize`, `content_type: String`
- `ArtifactMeta` struct: `name: String`, `latest_version: u32`, `created_at: DateTime<Utc>`, `updated_at: DateTime<Utc>`, `content_type: String`
- `ArtifactStore` trait: `save`, `load`, `load_version`, `list`, `delete` — all async, `Send + Sync` bounds
- `ArtifactError` enum: `InvalidName`, `StorageError`, `NotConfigured`
- `AgentEvent::ArtifactSaved` variant: `session_id: String`, `name: String`, `version: u32`
- Artifact name validation function (alphanumeric, hyphens, underscores, dots, forward slashes)
- Feature gate: `artifact-store` on core crate

### Phase 2: Artifacts Crate — InMemoryArtifactStore (P1 — FR-018, FR-019)

New `artifacts/` workspace crate with `InMemoryArtifactStore`:

- `HashMap<String, HashMap<String, Vec<(ArtifactVersion, ArtifactData)>>>` — session → artifact name → versions
- `tokio::sync::Mutex` for interior mutability (concurrent-safe, matches `Send + Sync`)
- `tracing::debug!` on save/load/list/delete operations
- Full test coverage: save, load latest, load specific version, list, delete, concurrent saves, empty cases

### Phase 3: Artifacts Crate — FileArtifactStore (P1 — FR-010, FR-011, FR-019)

Filesystem implementation:

```text
{root}/{session_id}/{artifact_name}/
├── meta.json            # ArtifactMeta + per-version records
├── v1.bin               # Version 1 content
├── v2.bin               # Version 2 content
└── ...
```

- Artifact names with forward slashes create subdirectories (e.g., `tool/output` → `{root}/{session}/{tool/output}/`)
- `meta.json` contains serialized version records + custom metadata per version
- Concurrent access: `tokio::sync::Mutex` per session+artifact key for version numbering; file writes use atomic temp-file + rename
- `tracing::info!` on save, `tracing::debug!` on load/list/delete
- Integration tests using `tempfile::TempDir`

### Phase 4: StreamingArtifactStore Extension Trait (P2 — FR-016)

- `StreamingArtifactStore` trait: `save_stream` (accepts `impl Stream<Item = Bytes>`) and `load_stream` (returns `impl Stream<Item = Bytes>`)
- `FileArtifactStore` implements it using `tokio::fs` read/write with buffered I/O
- `InMemoryArtifactStore` does NOT implement it (uses base `Vec<u8>` API only)
- Tests: 10MB artifact save/load via streaming

### Phase 5: Built-in Artifact Tools (P2 — FR-013, FR-017)

Three tools behind `artifact-tools` feature gate:

- **SaveArtifactTool**: Takes `name`, `content` (string), `content_type` (optional, defaults to `text/plain`). Captures `Arc<dyn ArtifactStore>`. Returns version number on success.
- **LoadArtifactTool**: Takes `name`, `version` (optional). Returns content as text for text types, size/type summary for binary.
- **ListArtifactsTool**: No required args. Returns formatted table of artifact names, versions, content types.
- All tools implement `AgentTool` with `JsonSchema`-derived parameter schemas
- `artifact_tools(store: Arc<dyn ArtifactStore>) -> Vec<Box<dyn AgentTool>>` convenience constructor

### Phase 6: Integration & Agent Configuration (P1 — FR-012, FR-015)

- Add optional `artifact_store: Option<Arc<dyn ArtifactStore>>` to `AgentOptions` (behind feature gate)
- Agent passes artifact store reference to event emission on artifact saves
- No changes to `AgentLoopConfig` or `StreamFn` — artifact store is orthogonal
- Verify agent operates normally when no artifact store configured

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| New workspace crate (`artifacts`) | Binary I/O with different perf characteristics than JSONL message store | Merging into `memory` crate couples binary storage to conversation persistence; different dependency needs (streaming I/O vs line-oriented JSON) |
