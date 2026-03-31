# Feature Specification: Context Management

**Feature Branch**: `006-context-management`
**Created**: 2026-03-20
**Updated**: 2026-03-31
**Status**: Draft
**Input**: Context windowing, transformation hooks, versioned history, message conversion pipeline, context caching, and overflow prediction. Manages how conversation history is pruned, transformed, cached, and prepared for LLM providers. References: PRD §5 (Agent Context), PRD §10.1 (Context Window Overflow), PRD §12.2 (Loop Config — transform_context, convert_to_llm), HLD Agent Context, Google ADK ContextCacheConfig, Anthropic cache_control blocks.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Automatic Context Pruning for Long Conversations (Priority: P1)

A developer runs a long conversation that grows beyond the model's context window. The system automatically prunes the message history using a sliding window strategy: it preserves an anchor set of early messages and a tail of recent messages, removing middle messages to fit within the budget. Tool-result pairs are kept together even if this slightly exceeds the budget.

**Why this priority**: Without context pruning, long conversations fail with context overflow errors. This is the primary mechanism for keeping conversations alive.

**Independent Test**: Can be tested by creating a message history that exceeds a token budget and verifying the pruning preserves anchor and tail while removing the middle, with tool-result pairs intact.

**Acceptance Scenarios**:

1. **Given** a conversation exceeding the token budget, **When** the sliding window is applied, **Then** anchor messages (first N) and tail messages (most recent) are preserved.
2. **Given** a conversation with tool call/result pairs in the middle, **When** pruning occurs, **Then** tool-result pairs are kept together — a tool result is never separated from its tool call.
3. **Given** a conversation within the token budget, **When** the sliding window is applied, **Then** no messages are removed.

---

### User Story 2 - Custom Context Transformation (Priority: P1)

A developer provides a custom transformation hook that runs before each LLM call. This hook can inject context (e.g., retrieved documents), prune messages, or apply any custom logic. Both synchronous and asynchronous variants are supported. When context overflow occurs, the hook receives an overflow signal so it can apply more aggressive pruning on retry.

**Why this priority**: Custom transformation is the extensibility point for advanced context management — RAG injection, summarization, custom pruning strategies. It's called on every turn.

**Independent Test**: Can be tested by providing a transformation hook that modifies the message list and verifying the modified context reaches the provider.

**Acceptance Scenarios**:

1. **Given** a synchronous transformation hook, **When** a turn begins, **Then** the hook is called with the current context before the conversion pipeline.
2. **Given** an asynchronous transformation hook, **When** a turn begins, **Then** the hook is awaited with the current context.
3. **Given** a context overflow on the previous attempt, **When** the transformation hook is called on retry, **Then** it receives the overflow signal.
4. **Given** no transformation hook configured, **When** a turn begins, **Then** the context passes through unchanged.

---

### User Story 3 - Message Conversion Pipeline (Priority: P1)

When preparing context for the LLM provider, the system converts each agent message to the provider's expected format. A conversion function maps each message, returning the converted form or nothing to filter it out. Custom application-defined messages are filtered out by default since they should never reach the provider.

**Why this priority**: The conversion pipeline is how custom and non-LLM messages are excluded from provider input. Without it, custom messages would cause provider errors.

**Independent Test**: Can be tested by creating a message history with standard and custom messages, applying the conversion, and verifying custom messages are filtered out.

**Acceptance Scenarios**:

1. **Given** a message history with standard messages, **When** the conversion function runs, **Then** each message is converted to the provider format.
2. **Given** a message history with custom application messages, **When** the conversion function runs, **Then** custom messages are filtered out (return nothing).
3. **Given** the conversion function, **When** it is called, **Then** it runs after the transformation hook on every turn.

---

### User Story 4 - Versioned Context History (Priority: P3)

A developer needs to track how the context evolves across turns for debugging or analysis. The system maintains versioned snapshots of the context, allowing inspection of what the agent saw at each turn boundary.

**Why this priority**: Context versioning is a debugging/observability feature — useful but not required for the agent to function.

**Independent Test**: Can be tested by running a multi-turn conversation and verifying that each turn's context snapshot can be retrieved.

**Acceptance Scenarios**:

1. **Given** a multi-turn conversation, **When** the context is versioned, **Then** each turn's context snapshot is independently retrievable.
2. **Given** context versions, **When** they are inspected, **Then** they show the progression of messages, transformations, and pruning across turns.

---

### User Story 5 - Explicit Context Caching with TTL (Priority: P1)

A developer configures context caching so that the static portion of the prompt (system instructions, tool descriptions) is cached provider-side across turns. This avoids re-tokenizing thousands of tokens every turn, reducing latency and cost. The framework splits context into cacheable (static) and non-cacheable (dynamic) portions. On the first turn the cacheable prefix is sent with cache control markers; subsequent turns reference the cached version until TTL expires or the cache interval is exceeded.

**Why this priority**: Provider-side caching (Anthropic prompt caching, Google context caching) is a major cost/latency optimization. Without framework support, every adapter must independently solve the static/dynamic split and cache lifecycle, leading to inconsistency and bugs.

**Independent Test**: Configure a `CacheConfig` with TTL=600s, min_tokens=4096, cache_intervals=3. Run 5 turns and verify: (a) turns 1-3 mark the prefix as cached, (b) turn 4 refreshes the cache, (c) the `CacheHint` annotations are present on the correct messages.

**Acceptance Scenarios**:

1. **Given** a `CacheConfig` on `AgentOptions`, **When** context is prepared for the provider, **Then** the framework splits messages into a cacheable prefix (static system prompt, tool descriptions) and a dynamic suffix (conversation history, dynamic prompt).
2. **Given** a cacheable prefix exceeding `min_tokens`, **When** the first turn is sent, **Then** the prefix carries a `CacheHint::Write` marker with the configured TTL.
3. **Given** a cached prefix within its TTL and `cache_intervals` count, **When** subsequent turns are sent, **Then** the prefix carries a `CacheHint::Read` marker (no re-send of full content).
4. **Given** the `cache_intervals` count is exceeded, **When** the next turn begins, **Then** the cache is refreshed with a new `CacheHint::Write`.
5. **Given** no `CacheConfig` is set, **When** context is prepared, **Then** no cache markers are added (backward compatible, zero overhead).

---

### User Story 6 - Static vs Dynamic Instruction Split (Priority: P1)

A developer provides both a static system prompt (persona, base instructions — stable across turns) and a dynamic system prompt (per-turn context like current time, user state — changes every turn). The static portion is the primary cache target. The dynamic portion is appended separately so it does not invalidate the cache.

**Why this priority**: This is the prerequisite for effective caching. Without separating static from dynamic instructions, any per-turn change in the system prompt invalidates the entire cache, negating the cost/latency benefit.

**Independent Test**: Create an agent with `static_system_prompt = "You are a helpful assistant..."` and `dynamic_system_prompt = Some(closure returning current timestamp)`. Verify the static portion remains unchanged across turns while the dynamic portion updates each turn.

**Acceptance Scenarios**:

1. **Given** both static and dynamic system prompts configured, **When** context is built for a turn, **Then** the static prompt is sent as the system prompt message (cacheable) and the dynamic prompt is injected as a separate user-role message immediately after it (non-cacheable).
2. **Given** only a `static_system_prompt` (no dynamic), **When** context is built, **Then** it behaves identically to the existing `system_prompt` field.
3. **Given** the dynamic prompt changes between turns, **When** context caching is active, **Then** only the dynamic portion changes — the cached static prefix is not invalidated.
4. **Given** neither static nor dynamic prompt is set but `system_prompt` is, **When** context is built, **Then** `system_prompt` is treated as static (backward compatible).

---

### User Story 7 - Context Overflow Predicate (Priority: P2)

A developer wants to check whether the current context would exceed the model's context window *before* sending the request. The framework provides `is_context_overflow(messages, model) -> bool` that estimates token count and compares against the model's `max_context_window` from `ModelCapabilities`. This allows pre-emptive compaction or user notification without a wasted round-trip.

**Why this priority**: Currently overflow is only detected after the provider returns an error (the `CONTEXT_OVERFLOW_SENTINEL` path). Pre-flight prediction avoids the failed request, saving latency and one wasted API call per overflow.

**Independent Test**: Create a message list with known token estimate, a `ModelSpec` with `max_context_window = Some(1000)`, and verify `is_context_overflow` returns true when estimated tokens > 1000 and false otherwise. Test with a custom `TokenCounter`.

**Acceptance Scenarios**:

1. **Given** messages whose estimated token count exceeds `model.capabilities.max_context_window`, **When** `is_context_overflow` is called, **Then** it returns `true`.
2. **Given** messages within the model's context window, **When** `is_context_overflow` is called, **Then** it returns `false`.
3. **Given** a model with `max_context_window = None` (unknown), **When** `is_context_overflow` is called, **Then** it returns `false` (cannot predict, let the provider decide).
4. **Given** a custom `TokenCounter`, **When** `is_context_overflow` is called with it, **Then** it uses the custom counter instead of the default chars/4 heuristic.

---

### Edge Cases

- What happens when the token budget is smaller than the anchor + one recent message — anchor messages are always preserved even if they exceed the budget. Correctness > token count.
- How does the system estimate tokens for custom messages that have no text content — custom messages are estimated at 100 tokens flat.
- What happens when the transformation hook adds messages that push the context over budget — the sliding window and the transform hook are independent; the hook is responsible for its own budget management. The sliding window is itself a type of transform hook.
- How does the system handle an empty conversation history — the sliding window returns no compaction (no-op). The agent validates empty history at the entry point level, returning `AgentError::NoMessages`.
- What happens when the cached prefix is compacted by the sliding window — the sliding window must not compact messages within the cached prefix. Anchor messages should encompass the cached prefix when caching is active.
- What if `min_tokens` is not met — no cache markers are emitted; the context is sent without caching (falls back to normal behavior).
- What if the provider does not support caching — adapters that don't support caching ignore `CacheHint` markers. The core emits them unconditionally; adapters opt in to honoring them.
- How does `is_context_overflow` interact with caching — it estimates the full context size (cached + dynamic) since that's what the model processes regardless of caching.
- Can `static_system_prompt` change mid-conversation — no. It is immutable for the lifetime of the agent loop. Changing it requires a new agent/session. This simplifies cache lifecycle and avoids stale-prefix bugs.
- What happens when the provider's cache expires between turns (TTL elapsed server-side) — the adapter returns a retryable cache-miss error. The framework detects it, resets `CacheState`, and retries with `CacheHint::Write`. Same retry path as `ModelThrottled`/`NetworkError`.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a sliding window context pruning strategy that preserves an anchor set (first N messages) and a tail set (most recent messages), removing middle messages to fit a token budget.
- **FR-002**: Sliding window MUST preserve tool call/result pairs together — a tool result MUST NOT be separated from its corresponding tool call, even if this slightly exceeds the budget.
- **FR-003**: System MUST provide a token estimation heuristic for determining message sizes.
- **FR-004**: Custom messages MUST be estimated at a flat token cost since they have no standard text content.
- **FR-005**: System MUST support a synchronous context transformation hook called before each provider call.
- **FR-006**: System MUST support an asynchronous context transformation hook as an alternative to the synchronous variant.
- **FR-007**: The transformation hook MUST receive an overflow signal when called after a context overflow error, enabling more aggressive pruning on retry.
- **FR-008**: System MUST provide a message conversion function that maps each agent message to the provider format, with the ability to filter messages by returning nothing.
- **FR-009**: The conversion function MUST run after the transformation hook on every turn.
- **FR-010**: System MUST support versioned context history that tracks context snapshots at turn boundaries.
- **FR-011**: System MUST support an optional `CacheConfig` that controls provider-side context caching with configurable TTL, minimum token threshold, and cache refresh interval.
- **FR-012**: When `CacheConfig` is active, the system MUST split context into a cacheable prefix (static system prompt, tool descriptions) and a non-cacheable suffix (dynamic prompt, conversation messages).
- **FR-013**: The system MUST annotate cacheable messages with `CacheHint::Write` on cache creation/refresh and `CacheHint::Read` on cache reuse, allowing adapters to translate to provider-specific cache control.
- **FR-014**: System MUST support separate `static_system_prompt` and `dynamic_system_prompt` fields, where the static portion is the primary cache target and the dynamic portion changes per turn without invalidating the cache.
- **FR-015**: System MUST provide an `is_context_overflow(messages, model, counter?) -> bool` predicate that estimates whether context exceeds the model's `max_context_window` before sending the request.
- **FR-016**: The sliding window MUST NOT compact messages within the cached prefix when caching is active.
- **FR-017**: When no `CacheConfig` is set, caching-related behavior MUST be completely absent — zero overhead, full backward compatibility.
- **FR-018**: When `CacheConfig` is active, the system MUST emit an `AgentEvent::CacheAction { hint, prefix_tokens }` event each turn, enabling observability of cache write/read/refresh transitions.
- **FR-019**: When an adapter returns a cache-miss error, the framework MUST reset `CacheState` and retry the turn with `CacheHint::Write`, following the existing `RetryStrategy` path.

### Key Entities

- **SlidingWindow**: Context pruning strategy — anchor set + tail set, middle removed, tool-result pairs preserved.
- **ContextTransformer**: Synchronous hook for rewriting context before each provider call.
- **AsyncContextTransformer**: Asynchronous variant of the context transformation hook.
- **ContextVersion**: Versioned snapshot of the context at a turn boundary.
- **ConvertToLlmFn**: Function that maps an agent message to a provider message or filters it out.
- **CacheConfig**: Configuration for provider-side context caching — TTL, min token threshold, cache interval.
- **CacheHint**: Enum annotation on messages indicating cache write/read intent for adapters.
- **CacheState**: Internal tracker for cache lifecycle — turn counter, cached prefix length.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Sliding window pruning correctly preserves anchor and tail messages while removing middle messages to fit within the budget.
- **SC-002**: Tool call/result pairs are never separated by pruning.
- **SC-003**: The transformation hook is called before the conversion function on every turn, in both sync and async variants.
- **SC-004**: The overflow signal is correctly propagated to the transformation hook after a context overflow error.
- **SC-005**: Custom messages are filtered out by the conversion pipeline and never reach the provider.
- **SC-006**: Token estimation produces consistent results for the same message content.
- **SC-007**: When `CacheConfig` is active with `cache_intervals=N`, the first turn emits `CacheHint::Write` and turns 2..N emit `CacheHint::Read`.
- **SC-008**: The static/dynamic prompt split produces a stable cacheable prefix that does not change between turns.
- **SC-009**: `is_context_overflow` returns true when estimated tokens exceed `max_context_window` and false otherwise, with zero false negatives when `max_context_window` is `None`.
- **SC-010**: An agent configured without `CacheConfig` behaves identically to pre-caching behavior — no extra allocations, no extra fields in messages.

## Clarifications

### Session 2026-03-20

- Q: What happens when budget is smaller than anchor messages? → A: Anchor messages always preserved even if they exceed budget. Correctness > token count.
- Q: How are custom messages estimated for tokens? → A: 100 tokens flat, regardless of content.
- Q: What if transform hook adds messages pushing over budget? → A: Sliding window and transform hook are independent; hook manages its own budget.
- Q: How does empty conversation history behave? → A: Sliding window is a no-op; agent returns `AgentError::NoMessages` at entry point.

### Session 2026-03-31

- Q: How do different providers implement caching? → A: Anthropic uses `cache_control: {"type": "ephemeral"}` blocks on messages. Google uses `CachedContent` API with explicit TTL. The framework abstracts this via `CacheHint` — adapters translate to their provider's format.
- Q: Where does `CacheHint` live — on messages or alongside them? → A: As a `cache_hint: Option<CacheHint>` field directly on `AgentMessage`, with `#[serde(default, skip_serializing_if = "Option::is_none")]` for zero-cost serialization when absent. Adapters inspect it during conversion. Messages without hints are sent normally.
- Q: Does `static_system_prompt` replace `system_prompt`? → A: No. `system_prompt` continues to work as-is (treated as static). The new fields are opt-in additions. If `static_system_prompt` is set, it takes precedence over `system_prompt` for the cached portion.
- Q: How does caching interact with the sliding window? → A: The cached prefix (static prompt content) must be excluded from sliding window compaction. When `CacheConfig` is active, the anchor count should be at least large enough to cover the cached prefix.
- Q: What happens to `CacheHint` markers for providers that don't support caching? → A: Adapters that don't support caching simply ignore the hints. The markers are a no-op for those adapters.
- Q: Should `is_context_overflow` account for tool schemas? → A: No — tool schemas are sent out-of-band in most providers. The predicate estimates message tokens only. Callers can add tool schema overhead manually if needed.
- Q: Who is responsible for TTL expiry — framework or provider? → A: Framework tracks turn count only (via `cache_intervals`); TTL is a provider-side hint passed through `CacheHint::Write { ttl }`. The provider handles actual time-based expiry. If the provider's cache expired between turns, the adapter receives an error and the framework falls back to a fresh `CacheHint::Write` on retry. `CacheState.is_valid` reflects turn-count validity only, not wall-clock TTL.
- Q: Should cache lifecycle transitions emit AgentEvents? → A: Yes. Emit `AgentEvent::CacheAction { hint: CacheHint, prefix_tokens: usize }` each turn when caching is active. Consistent with existing `ContextCompacted` pattern. No event emitted when `CacheConfig` is absent.
- Q: How is the dynamic prompt placed relative to the static prompt? → A: Static prompt is the system prompt message (the cache target). Dynamic prompt is injected as a separate user-role message immediately after the system prompt. This ensures the system prompt message is byte-identical across turns, preserving the cache. Follows Google ADK's pattern: static in `system_instruction`, dynamic appended to `contents`.
- Q: Can `static_system_prompt` change mid-conversation? → A: No. `static_system_prompt` is immutable for the lifetime of the agent loop. Changing persona or tool set requires starting a new agent/session. This is a documented constraint — enforced by taking `static_system_prompt` as an owned `String` at construction, not a closure.
- Q: How does the adapter signal a provider-side cache miss to the framework? → A: Adapter returns a retryable error (new `AgentError::CacheMiss` variant or tagged `NetworkError`). The framework's retry path detects the cache-miss error kind and calls `CacheState::reset()` before the retry turn, so the next attempt emits `CacheHint::Write` with fresh content. Follows the existing `RetryStrategy` pattern — no new retry mechanism needed.

## Assumptions

- Token estimation uses a characters-divided-by-4 heuristic as an approximation. Exact tokenization is not required.
- Custom messages are estimated at 100 tokens flat.
- The sliding window anchor size and tail size are configurable by the caller.
- Context transformation is synchronous by default; the async variant is an opt-in alternative.
- Versioned context history is opt-in and does not impose overhead when not used.
- Context caching is opt-in via `CacheConfig`. When not configured, no cache-related types are allocated or processed.
- The `CacheHint` abstraction is provider-agnostic — the core does not know about Anthropic's `cache_control` or Google's `CachedContent` API.
- `is_context_overflow` is a best-effort estimate. It uses the same `TokenCounter` as the sliding window and may under/over-estimate compared to the provider's exact tokenizer.
- `static_system_prompt` and `dynamic_system_prompt` are optional fields that coexist with the existing `system_prompt` for backward compatibility.
- `static_system_prompt` is immutable for the agent loop lifetime. Mid-conversation persona/tool changes require a new agent session.
