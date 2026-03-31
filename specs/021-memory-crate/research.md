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

### 8. Rich Session Entry Types via Tagged Enum

**Decision**: Introduce `SessionEntry` as a serde-tagged enum with `#[serde(tag = "entry_type")]`. Old-format JSONL lines (without `entry_type`) are deserialized as `SessionEntry::Message` via a custom deserializer fallback.

**Rationale**: A tagged enum provides compile-time exhaustiveness and serde compatibility. The `entry_type` discriminator is minimal overhead (one extra field per line). Backward compatibility with old sessions is achieved by checking whether the `entry_type` field exists — if absent, the line is assumed to be a `Message`. This avoids requiring migration of existing session files.

**Key reference**: Pi Agent's session entry types (message, model_change, thinking_level_change, compaction, label, custom).

**Alternatives Rejected**:
- **Separate files per entry type**: Loses ordering; entries must interleave chronologically.
- **Wrapper struct with optional fields**: Loses type safety; every field is `Option` and the correct combination is runtime-checked.
- **Binary format with type tags**: Not human-readable; violates the JSONL design decision.

### 9. Interrupt State as Separate JSON File

**Decision**: Persist interrupt state as `{session_id}.interrupt.json`, not inline in the JSONL stream.

**Rationale**: Interrupt state is transient — it represents a snapshot at a point in time that should be consumed (resumed) and then deleted. Putting it in the JSONL stream would make it permanent, requiring special handling during load to distinguish "active interrupt" from "historical interrupt record." A separate file is simpler: exists = interrupted, deleted = resumed. The JSON format (not JSONL) is appropriate because interrupt state is a single structured object, not a stream.

**Alternatives Rejected**:
- **Inline in JSONL**: Mixes transient state with permanent log; complicates load logic.
- **Database table**: Adds dependency; overkill for a single JSON object.
- **In-memory only (no persistence)**: Loses state on crash — defeats the purpose.

### 10. Optimistic Concurrency via Sequence Counter

**Decision**: Add `sequence: u64` to `SessionMeta`, incremented on every write. On save, optionally check that the stored sequence matches the expected value.

**Rationale**: File locking is complex and cross-platform-inconsistent (decision 5). A sequence counter provides lightweight optimistic concurrency — the second writer detects the conflict and can retry or report. This is optional: callers who don't care about concurrency ignore the sequence. The implementation is simple: read sequence on load, pass it back on save, compare before writing.

**Alternatives Rejected**:
- **File locking (flock)**: Cross-platform differences; advisory on some systems; deadlock risk.
- **Last-write-wins only**: Silently loses data; acceptable for single-writer but dangerous for multi-writer.
- **CAS with external lock service**: Massive over-engineering for a local filesystem store.

### 11. Filtered Loading via LoadOptions

**Decision**: Add `LoadOptions` parameter to `load()` with `last_n_entries`, `after_timestamp`, and `entry_types` filters. For JSONL, implement by reading the full file and filtering in memory (with future optimization to seek-from-end for `last_n_entries`).

**Rationale**: Large sessions (thousands of entries) are expensive to load fully when only recent context is needed. The filtering API is simple and composable. The initial implementation reads and filters in memory — this is correct and sufficient for typical session sizes (<10K entries). Seek-from-end optimization can be added later without API changes.

**Alternatives Rejected**:
- **Separate "load_last_n" method**: Proliferates trait methods. A single `load_with_options` is cleaner.
- **Streaming iterator**: More complex API; callers typically want all matching entries in a `Vec`.
- **Index file for random access**: Significant complexity; premature optimization.

## Open Questions

None — all clarifications resolved in the spec.
