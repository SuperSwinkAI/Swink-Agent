# Research: Context Management

**Feature**: 006-context-management | **Date**: 2026-03-20

## Design Decisions

### D1: Sliding window algorithm with anchor + tail strategy

**Decision**: The sliding window preserves a configurable number of anchor messages (first N) and fills the remaining budget with tail messages (most recent), removing the contiguous middle section. Walking backward from the end accumulates messages that fit within the remaining budget after anchors.

**Rationale**: Anchor messages typically contain the system prompt response or initial user request that defines the conversation goal. Tail messages contain the most recent context the model needs to continue. The middle is the least valuable for continuation. This is the same strategy used by Claude Code and similar agent systems.

**Alternatives rejected**:
- **Summarization-only**: Requires an LLM call during compaction, adding latency and cost on every overflow. The sliding window is O(n) and synchronous.
- **Random eviction**: Destroys conversation coherence.
- **LRU / importance scoring**: Adds complexity without clear benefit. The anchor+tail heuristic works well in practice.

### D2: Token estimation via chars/4 heuristic

**Decision**: Token count is estimated by summing character lengths of all text-bearing content blocks and dividing by 4. No tokenizer library dependency.

**Rationale**: The chars/4 ratio is a widely-used approximation for English text with common tokenizers (BPE, SentencePiece). Exact token counts vary by provider and model, so any heuristic is inherently approximate. The simplicity avoids a tokenizer dependency in the core crate while giving "good enough" budget enforcement.

**Alternatives rejected**:
- **tiktoken / tokenizers crate**: Adds a heavy dependency to the core crate. Provider-specific (tiktoken is OpenAI's tokenizer). The `TokenCounter` trait allows users to plug in exact counting when needed.
- **No estimation**: Would require callers to manage budget externally, defeating the purpose of automatic context pruning.

### D3: Custom message flat cost of 100 tokens

**Decision**: `CustomMessage` variants are estimated at 100 tokens regardless of content.

**Rationale**: Custom messages have no standard text content (they implement `CustomMessage` trait with `as_any()`). A fixed cost prevents them from being "free" in budget calculations (which would let unbounded custom messages accumulate). 100 tokens is a conservative middle ground -- large enough to trigger compaction if many accumulate, small enough to not over-penalize a few.

### D4: Synchronous transform as default, async as opt-in alternative

**Decision**: `ContextTransformer` is a synchronous trait. `AsyncContextTransformer` is a separate async trait. The sync variant is used by default; the async variant is opt-in via `AgentOptions::with_async_transform_context()`.

**Rationale**: The transform hook runs on every turn in the hot loop. Most transforms (sliding window, simple injection) are CPU-bound and benefit from synchronous execution with no async overhead. The async variant exists for transforms that need I/O (RAG retrieval, LLM-based summarization) but is opt-in to avoid forcing all users into async.

**Alternatives rejected**:
- **Async-only**: Forces all transforms to be async even when they are trivially synchronous, adding unnecessary `.await` overhead and complexity.
- **Single trait with both sync and async methods**: Awkward API with default implementations that would need to choose one path.

### D5: Context versioning via VersioningTransformer wrapper

**Decision**: `VersioningTransformer` wraps an inner `ContextTransformer` and captures dropped messages as `ContextVersion` snapshots, storing them in a pluggable `ContextVersionStore`. Version numbers are monotonically increasing. An optional `ContextSummarizer` produces summaries for each version.

**Rationale**: Versioning is orthogonal to the compaction algorithm. Wrapping rather than embedding keeps the sliding window simple and allows versioning to work with any transformer, including user-provided ones. The `ContextVersionStore` trait enables both in-memory (testing/single-session) and persistent (database/file) storage without coupling the core crate to a storage backend.

**Alternatives rejected**:
- **Built-in versioning in the sliding window**: Couples two concerns. Users who don't need versioning would pay for snapshot overhead.
- **Event-based versioning**: Would require a separate listener to capture dropped messages, adding indirection and making it harder to correlate versions with specific compaction passes.

### D6: Message conversion via MessageConverter trait + convert_messages generic function

**Decision**: Each adapter implements the `MessageConverter` trait (with associated `Message` type) to supply format-specific conversion. The generic `convert_messages<C>()` function handles iteration, pattern matching, and custom message filtering. Custom messages (`AgentMessage::Custom`) are silently skipped.

**Rationale**: All adapters share the same iteration pattern: optional system message, then map each `LlmMessage` variant. Factoring this into a generic function eliminates boilerplate duplication across adapters while letting each adapter define its own message type.

**Alternatives rejected**:
- **Closure-based conversion (ConvertToLlmFn)**: The original design used a bare closure. The trait approach is more structured and provides compile-time type checking for the message type.
- **Enum-based conversion**: Would require a single message enum that all providers share, which is too constraining for providers with different capabilities.
