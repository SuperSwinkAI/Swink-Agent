# Research: Memory Crate

**Branch**: `021-memory-crate` | **Date**: 2026-03-20

## Design Decisions

### 1. JSONL Storage Format

**Decision**: Use JSONL (JSON Lines) where line 1 is `SessionMeta` and lines 2+ are `LlmMessage` values.

**Rationale**: JSONL provides append-only writes (new messages appended without rewriting), human readability for debugging, and partial corruption recovery (a bad line loses one message, not the session). Each line is independently parseable.

**Alternatives Rejected**:
- **Single JSON file**: Requires full rewrite on every save; no partial recovery.
- **SQLite**: Adds a native dependency; overkill for single-writer local storage.
- **MessagePack/bincode**: Not human-readable; harder to debug.

### 2. Sync + Async Store Traits

**Decision**: Define both `SessionStore` (sync) and `AsyncSessionStore` (async) as separate traits.

**Rationale**: The sync trait serves simple CLI use cases and tests without requiring a Tokio runtime. The async trait serves server-side and concurrent use cases. `JsonlSessionStore` implements both — sync uses `std::fs`, async uses `tokio::fs`.

**Alternatives Rejected**:
- **Async-only with `block_on`**: Forces a runtime dependency even for simple scripts; `block_on` inside an existing runtime panics.
- **Sync-only with `spawn_blocking`**: Callers must wrap every call; ergonomically poor.

### 3. Summarization via External LLM Call

**Decision**: `SummarizingCompactor` accepts a summarization function (`async Fn(Vec<LlmMessage>) -> Result<String>`) rather than embedding a specific provider.

**Rationale**: The memory crate must remain provider-agnostic (Constitution V). The compactor orchestrates *when* and *what* to summarize; the *how* is the caller's responsibility. This keeps the crate free of API keys, SDK clients, and provider-specific types.

**Pattern**: Pre-compute summaries asynchronously after each turn via `set_summary()`. The `compaction_fn()` closure (used by `Agent::with_transform_context()`) is synchronous and injects the pre-computed summary. This respects the constraint that `TransformContextFn` is synchronous.

### 4. Sliding Window Wrapper

**Decision**: The compactor uses a sliding window approach — recent messages are kept verbatim, older messages are replaced by a summary injected as an `AssistantMessage`.

**Rationale**: Matches the core crate's existing context management pattern (anchor + tail). The summary serves as a synthetic anchor. Injecting as `AssistantMessage` maintains user/assistant alternation since conversations start with a user message.

**Alternatives Rejected**:
- **Hierarchical summaries (summaries of summaries)**: Added complexity with diminishing returns for typical conversation lengths.
- **Embedding-based retrieval**: Out of scope per PRD non-goals; belongs in a future advanced memory system.

### 5. Single-Writer Assumption

**Decision**: No file locking. Last-writer-wins semantics. Single-writer assumption is documented.

**Rationale**: The primary use case is a single developer running a single agent process. File locking adds complexity (cross-platform differences, deadlock risk) for a scenario that rarely occurs. If concurrent writes corrupt a file, JSONL partial recovery limits damage.

**Alternatives Rejected**:
- **Advisory file locks (`flock`/`LockFile`)**: Cross-platform complexity; still advisory on some systems.
- **Write-ahead log**: Significant complexity for marginal benefit in a single-user tool.

### 6. Session ID Validation

**Decision**: Validate session IDs — reject `/`, `\`, `..`, and null bytes with a clear error. Auto-generated IDs use `YYYYMMDD_HHMMSS` format.

**Rationale**: Session IDs map directly to filenames. Unsafe characters enable path traversal attacks or filesystem errors. Validation at the API boundary prevents these issues.

### 7. CustomMessage Filtering

**Decision**: `CustomMessage` values are filtered out during serialization — they are not persisted.

**Rationale**: `CustomMessage` is an opaque type used for in-flight control signals. It survives compaction but never reaches the provider and has no meaningful serialization. Filtering matches the core crate's `in_flight_llm_messages` behavior.

## Open Questions

None — all clarifications resolved in the spec.
