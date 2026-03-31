# Data Model: Context Management

**Feature**: 006-context-management | **Date**: 2026-03-20 | **Updated**: 2026-03-31

## Entities

### SlidingWindow (function-based)

Context pruning strategy created via `sliding_window()`. Returned as a closure implementing `Fn(&mut Vec<AgentMessage>, bool) + Send + Sync`.

| Parameter | Type | Description |
|-----------|------|-------------|
| `normal_budget` | `usize` | Token budget under normal operation |
| `overflow_budget` | `usize` | Smaller budget used after context overflow |
| `anchor` | `usize` | Number of messages at the start to always preserve |

### SlidingWindowTransformer

Struct-based sliding window that implements `ContextTransformer` with compaction reporting.

| Field | Type | Description |
|-------|------|-------------|
| `normal_budget` | `usize` | Token budget under normal operation |
| `overflow_budget` | `usize` | Smaller budget used after context overflow |
| `anchor` | `usize` | Number of messages at the start to always preserve |
| `token_counter` | `Option<Arc<dyn TokenCounter>>` | Pluggable token estimation (default: chars/4) |

### CompactionResult

Returned by `compact_sliding_window()` when messages were dropped.

| Field | Type | Description |
|-------|------|-------------|
| `dropped_count` | `usize` | Number of messages removed |
| `tokens_before` | `usize` | Estimated tokens before compaction |
| `tokens_after` | `usize` | Estimated tokens after compaction |

### CompactionReport

Returned by `ContextTransformer::transform()` when messages were modified.

| Field | Type | Description |
|-------|------|-------------|
| `dropped_count` | `usize` | Number of messages removed during compaction |
| `tokens_before` | `usize` | Estimated tokens before compaction |
| `tokens_after` | `usize` | Estimated tokens after compaction |
| `overflow` | `bool` | Whether compaction was triggered by overflow |

### TokenCounter (trait)

Pluggable token counting strategy.

| Method | Signature | Description |
|--------|-----------|-------------|
| `count_tokens` | `(&self, &AgentMessage) -> usize` | Estimated token count for a single message |

### DefaultTokenCounter

Built-in implementation of `TokenCounter`.

| Message Type | Estimation Rule |
|-------------|-----------------|
| `LlmMessage` | Sum character lengths of all text-bearing content blocks, divide by 4 |
| `CustomMessage` | 100 tokens flat |

### ContextTransformer (trait)

Synchronous context transformation hook.

| Method | Signature | Description |
|--------|-----------|-------------|
| `transform` | `(&self, &mut Vec<AgentMessage>, bool) -> Option<CompactionReport>` | Transform context in-place; `bool` is overflow signal |

Blanket implementation: any `Fn(&mut Vec<AgentMessage>, bool) + Send + Sync` implements `ContextTransformer` automatically.

### AsyncContextTransformer (trait)

Asynchronous context transformation hook.

| Method | Signature | Description |
|--------|-----------|-------------|
| `transform` | `(&self, &mut Vec<AgentMessage>, bool) -> Pin<Box<dyn Future<Output = Option<CompactionReport>> + Send>>` | Async transform; `bool` is overflow signal |

### ContextVersion

Snapshot of messages captured during compaction.

| Field | Type | Description |
|-------|------|-------------|
| `version` | `u64` | Monotonically increasing version number (starts at 1) |
| `turn` | `u64` | Turn number when this version was created |
| `timestamp` | `u64` | Unix timestamp (seconds) |
| `messages` | `Vec<LlmMessage>` | LLM messages dropped during compaction |
| `summary` | `Option<String>` | Optional pre-computed summary |

### ContextVersionMeta

Lightweight metadata for listing versions without loading message content.

| Field | Type | Description |
|-------|------|-------------|
| `version` | `u64` | Version number |
| `turn` | `u64` | Turn number when created |
| `timestamp` | `u64` | Unix timestamp |
| `message_count` | `usize` | Number of messages in this version |
| `has_summary` | `bool` | Whether a summary is available |

### ContextVersionStore (trait)

Pluggable storage for version snapshots.

| Method | Signature | Description |
|--------|-----------|-------------|
| `save_version` | `(&self, &ContextVersion)` | Persist a version |
| `load_version` | `(&self, u64) -> Option<ContextVersion>` | Load by version number |
| `list_versions` | `(&self) -> Vec<ContextVersionMeta>` | List all version metadata |
| `latest_version` | `(&self) -> Option<ContextVersion>` | Load most recent (default impl) |

### InMemoryVersionStore

In-memory implementation of `ContextVersionStore` for testing and single-session use.

| Field | Type | Description |
|-------|------|-------------|
| `versions` | `Mutex<Vec<ContextVersion>>` | Thread-safe storage (poison-recovered) |

### ContextSummarizer (trait)

Synchronous summarization of dropped messages.

| Method | Signature | Description |
|--------|-----------|-------------|
| `summarize` | `(&self, &[LlmMessage]) -> Option<String>` | Produce a summary of dropped messages |

### VersioningTransformer

Wraps an inner `ContextTransformer` and captures dropped messages as versioned snapshots.

| Field | Type | Description |
|-------|------|-------------|
| `inner` | `Box<dyn ContextTransformer>` | Wrapped transformer (e.g., sliding window) |
| `store` | `Arc<dyn ContextVersionStore>` | Version storage backend |
| `summarizer` | `Option<Arc<dyn ContextSummarizer>>` | Optional summarizer |
| `state` | `Mutex<VersioningState>` | Internal versioning state (next_version, turn_counter) |

### MessageConverter (trait)

Provider-specific message conversion.

| Associated Type | Description |
|----------------|-------------|
| `Message` | The provider-specific message type |

| Method | Signature | Description |
|--------|-----------|-------------|
| `system_message` | `(system_prompt: &str) -> Option<Self::Message>` | Convert system prompt (None if out-of-band) |
| `user_message` | `(&UserMessage) -> Self::Message` | Convert user message |
| `assistant_message` | `(&AssistantMessage) -> Self::Message` | Convert assistant message |
| `tool_result_message` | `(&ToolResultMessage) -> Self::Message` | Convert tool result |

### ConvertToLlmFn (type alias)

Legacy closure-based conversion function used by `AgentOptions`.

```rust
type ConvertToLlmFn = Arc<dyn Fn(&[AgentMessage], &str) -> Vec<LlmMessage> + Send + Sync>;
```

### CacheConfig

Configuration for provider-side context caching. Optional on `AgentOptions`.

| Field | Type | Description |
|-------|------|-------------|
| `ttl` | `Duration` | How long cached content remains valid provider-side |
| `min_tokens` | `usize` | Minimum estimated token count to justify caching (below this, no cache markers emitted) |
| `cache_intervals` | `usize` | Reuse the same cache for N turns before refreshing with a new `Write` |

### CacheHint (enum)

Annotation on messages indicating cache intent. Adapters translate this to provider-specific format (e.g., Anthropic `cache_control`, Google `CachedContent`).

| Variant | Fields | Description |
|---------|--------|-------------|
| `Write` | `ttl: Duration` | Mark this content for caching with the given TTL |
| `Read` | — | Reference previously cached content (don't re-send full payload) |

### CacheState

Internal state tracker for cache lifecycle. Managed by the context pipeline, not user-facing. Tracks turn count only — TTL expiry is the provider's responsibility.

| Field | Type | Description |
|-------|------|-------------|
| `turns_since_write` | `usize` | Turns elapsed since last `CacheHint::Write` |
| `cached_prefix_len` | `usize` | Number of messages in the cached prefix (protected from compaction) |

### SystemPromptConfig

Replaces the single `system_prompt` string with a static/dynamic split for caching support.

| Field | Type | Description |
|-------|------|-------------|
| `static_system_prompt` | `String` | Stable instructions cached across turns (persona, base behavior, tool descriptions) |
| `dynamic_system_prompt` | `Option<Box<dyn Fn() -> String + Send + Sync>>` | Per-turn context generator (current time, user state, etc.) — `None` if no dynamic content |

## Functions

### is_context_overflow

Pre-flight predicate that estimates whether the current context would exceed the model's context window.

| Parameter | Type | Description |
|-----------|------|-------------|
| `messages` | `&[AgentMessage]` | Current context messages |
| `model` | `&ModelSpec` | Model spec with `capabilities.max_context_window` |
| `token_counter` | `Option<&dyn TokenCounter>` | Optional custom counter (defaults to `DefaultTokenCounter`) |
| **Returns** | `bool` | `true` if estimated tokens exceed `max_context_window`; `false` if within budget or window is unknown |

## Relationships

```
sliding_window() --creates--> closure implementing ContextTransformer (blanket impl)
SlidingWindowTransformer --implements--> ContextTransformer
SlidingWindowTransformer --uses--> TokenCounter (optional, defaults to DefaultTokenCounter)
VersioningTransformer --wraps--> Box<dyn ContextTransformer>
VersioningTransformer --stores-into--> Arc<dyn ContextVersionStore>
VersioningTransformer --optionally-uses--> Arc<dyn ContextSummarizer>
InMemoryVersionStore --implements--> ContextVersionStore
AsyncContextTransformer --parallel-to--> ContextTransformer (async variant)
MessageConverter --used-by--> convert_messages() (generic function)
AgentOptions --configures--> ContextTransformer | AsyncContextTransformer | ConvertToLlmFn
AgentOptions --optionally-configures--> CacheConfig
AgentOptions --optionally-configures--> SystemPromptConfig (static + dynamic)
CacheConfig --controls--> CacheState (internal lifecycle)
CacheState --emits--> CacheHint (on messages)
CacheHint --consumed-by--> Adapters (MessageConverter impls)
is_context_overflow() --uses--> TokenCounter + ModelSpec.capabilities.max_context_window
SlidingWindowTransformer --respects--> CacheState.cached_prefix_len (skip compaction of cached prefix)
```
