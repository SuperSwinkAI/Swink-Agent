# Research: Context Management

**Feature**: 006-context-management | **Date**: 2026-03-20 | **Updated**: 2026-03-31

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

### D7: Provider-agnostic CacheHint abstraction for context caching

**Decision**: The core framework defines a `CacheHint` enum (`Write { ttl }` | `Read`) that annotates messages in the context. Adapters translate these hints to provider-specific cache control mechanisms. A `CacheConfig` struct configures the cache lifecycle (TTL, min_tokens threshold, refresh interval). Internal `CacheState` tracks turns since last write and determines when to emit Write vs Read hints.

**Rationale**: Provider-side context caching is one of the highest-impact cost/latency optimizations available. Anthropic uses inline `cache_control: {"type": "ephemeral"}` blocks. Google uses a `CachedContent` API with server-side TTL. These mechanisms are fundamentally different but share the same semantic: "this prefix is stable, cache it." The framework abstracts the *intent* (what to cache, when to refresh) while adapters own the *mechanism* (how to tell the provider). This follows the same pattern as `MessageConverter` — core defines structure, adapters implement details.

**Key design choice — CacheHint as annotation, not message wrapper**: The hint is an optional field/annotation on messages rather than a wrapping type. This keeps the existing `AgentMessage` pipeline unchanged. Messages without hints flow through untouched. Adapters that don't support caching ignore hints — zero behavioral change.

**Reference implementations studied**:
- **Google ADK**: `ContextCacheConfig(min_tokens=4096, ttl_seconds=600, cache_intervals=3)` at the application level. Static instruction goes in `system_instruction` (cacheable), dynamic instruction appended to user contents (not cached). Prompt structure: `system_instruction + tools + tool_config` = cached prefix, `contents` = dynamic.
- **Anthropic SDK**: `cache_control: {"type": "ephemeral"}` on individual content blocks within messages. No explicit TTL (provider manages eviction). Implicit caching in newer models.

**Alternatives rejected**:
- **Adapter-only caching**: Each adapter independently implements caching. Leads to inconsistent behavior, duplicated lifecycle logic, and no way for the core to reason about cache boundaries during compaction.
- **Transparent caching (hidden from user)**: Automatically cache everything above a threshold. Too opaque — users need control over what's cached and when to refresh, especially when system prompts change.
- **Cache as a separate pipeline stage**: A dedicated "cache transformer" in the transform chain. Adds pipeline complexity for something that's really a message annotation concern, not a message mutation concern.

### D8: Static vs dynamic system prompt separation

**Decision**: `AgentOptions` gains optional `static_system_prompt: String` and `dynamic_system_prompt: Option<Box<dyn Fn() -> String + Send + Sync>>` fields. The existing `system_prompt` field remains and is treated as static when the new fields are not set. When `static_system_prompt` is set, it takes precedence for the cached portion. The dynamic prompt is a closure called each turn, producing fresh context.

**Rationale**: Following Google ADK's pattern: the static portion (`system_instruction`) is placed in the cacheable prefix, while dynamic content is appended to user contents outside the cache boundary. This is the single most effective thing a framework can do for caching — ensure the cacheable prefix is actually stable. A closure for dynamic content (rather than a string) lets per-turn context be generated lazily without requiring the caller to manually update the prompt each turn.

**Alternatives rejected**:
- **Single system_prompt with cache markers**: Users would need to manually split their prompt and annotate sections. Error-prone and not ergonomic.
- **Enum-based prompt type**: `SystemPrompt::Static(String) | SystemPrompt::Dynamic(Fn)`. Doesn't support the mixed case where you want both static and dynamic portions.

### D9: Pre-flight context overflow predicate

**Decision**: A standalone function `is_context_overflow(messages, model, counter?) -> bool` estimates the total token count of the message list using the provided (or default) `TokenCounter` and compares against `model.capabilities.max_context_window`. Returns `false` when `max_context_window` is `None` (unknown — let the provider decide).

**Rationale**: The current overflow detection relies on the provider returning an error (`CONTEXT_OVERFLOW_SENTINEL`), which wastes a round-trip. A pre-flight check avoids this. The function is deliberately simple — it doesn't trigger compaction or modify state, just answers "will this fit?" Callers can use it to decide whether to compact, warn the user, or proceed. Returning `false` for unknown context windows is the safe default: better to attempt and let the provider reject than to falsely block requests for models whose limits we don't know.

**Alternatives rejected**:
- **Automatic pre-flight compaction**: Check and auto-compact in one step. Too much magic — callers should decide what to do when overflow is predicted.
- **Result type with token details**: Return `OverflowInfo { estimated_tokens, max_tokens, overflow: bool }`. Over-engineered for the common case. The bool is the 90% use case; callers who want details can call `estimate_tokens` directly.
