# Public API Contract: Context Management

**Feature**: 006-context-management | **Date**: 2026-03-20

## Sliding Window

```rust
// Create a sliding window compaction closure
sliding_window(normal_budget: usize, overflow_budget: usize, anchor: usize)
    -> impl Fn(&mut Vec<AgentMessage>, bool) + Send + Sync

// Core compaction algorithm (default token counter)
compact_sliding_window(messages: &mut Vec<AgentMessage>, budget: usize, anchor: usize)
    -> Option<CompactionResult>

// Compaction with pluggable token counter
compact_sliding_window_with(
    messages: &mut Vec<AgentMessage>,
    budget: usize,
    anchor: usize,
    counter: Option<&dyn TokenCounter>,
) -> Option<CompactionResult>
```

## Token Estimation

```rust
// Estimate tokens for a single message (chars/4 for LLM, 100 flat for Custom)
estimate_tokens(msg: &AgentMessage) -> usize
```

## TokenCounter Trait

```rust
trait TokenCounter: Send + Sync {
    fn count_tokens(&self, message: &AgentMessage) -> usize;
}

// Built-in implementation
struct DefaultTokenCounter;  // derives Debug, Clone, Copy, Default
```

## ContextTransformer Trait

```rust
trait ContextTransformer: Send + Sync {
    fn transform(
        &self,
        messages: &mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Option<CompactionReport>;
}

// Blanket impl for closures:
// impl<F: Fn(&mut Vec<AgentMessage>, bool) + Send + Sync> ContextTransformer for F
```

## SlidingWindowTransformer

```rust
SlidingWindowTransformer::new(normal_budget: usize, overflow_budget: usize, anchor: usize)
    -> SlidingWindowTransformer

// Builder method
.with_token_counter(counter: Arc<dyn TokenCounter>) -> Self
```

## CompactionReport

```rust
struct CompactionReport {
    pub dropped_count: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub overflow: bool,
}
```

## CompactionResult

```rust
struct CompactionResult {
    pub dropped_count: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
}
```

## AsyncContextTransformer Trait

```rust
trait AsyncContextTransformer: Send + Sync {
    fn transform<'a>(
        &'a self,
        messages: &'a mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Pin<Box<dyn Future<Output = Option<CompactionReport>> + Send + 'a>>;
}
```

## Context Versioning

```rust
// Snapshot type
struct ContextVersion {
    pub version: u64,
    pub turn: u64,
    pub timestamp: u64,
    pub messages: Vec<LlmMessage>,
    pub summary: Option<String>,
}

// Metadata type
struct ContextVersionMeta {
    pub version: u64,
    pub turn: u64,
    pub timestamp: u64,
    pub message_count: usize,
    pub has_summary: bool,
}
```

## ContextVersionStore Trait

```rust
trait ContextVersionStore: Send + Sync {
    fn save_version(&self, version: &ContextVersion);
    fn load_version(&self, version: u64) -> Option<ContextVersion>;
    fn list_versions(&self) -> Vec<ContextVersionMeta>;
    fn latest_version(&self) -> Option<ContextVersion>;  // default impl
}
```

## InMemoryVersionStore

```rust
InMemoryVersionStore::new() -> Self        // const fn
InMemoryVersionStore::len(&self) -> usize
InMemoryVersionStore::is_empty(&self) -> bool
```

## ContextSummarizer Trait

```rust
trait ContextSummarizer: Send + Sync {
    fn summarize(&self, messages: &[LlmMessage]) -> Option<String>;
}
```

## VersioningTransformer

```rust
VersioningTransformer::new(
    inner: impl ContextTransformer + 'static,
    store: Arc<dyn ContextVersionStore>,
) -> Self

// Builder methods
.with_summarizer(summarizer: Arc<dyn ContextSummarizer>) -> Self

// Accessor
.store(&self) -> &Arc<dyn ContextVersionStore>
```

## Message Conversion

```rust
trait MessageConverter {
    type Message;

    fn system_message(system_prompt: &str) -> Option<Self::Message>;
    fn user_message(user: &UserMessage) -> Self::Message;
    fn assistant_message(assistant: &AssistantMessage) -> Self::Message;
    fn tool_result_message(result: &ToolResultMessage) -> Self::Message;
}

// Generic conversion function (skips CustomMessage variants)
convert_messages<C: MessageConverter>(
    messages: &[AgentMessage],
    system_prompt: &str,
) -> Vec<C::Message>
```

## Tool Schema Extraction

```rust
struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

extract_tool_schemas(tools: &[Arc<dyn AgentTool>]) -> Vec<ToolSchema>
```

## AgentOptions Builder Methods (context-related)

```rust
.with_transform_context(transformer: impl ContextTransformer + 'static) -> Self
.with_transform_context_fn(closure: impl Fn(&mut Vec<AgentMessage>, bool) + Send + Sync + 'static) -> Self
.with_async_transform_context(transformer: impl AsyncContextTransformer + 'static) -> Self
.with_token_counter(counter: Arc<dyn TokenCounter>) -> Self
```

## Re-exports from lib.rs

```rust
pub use context::{DefaultTokenCounter, TokenCounter, estimate_tokens, sliding_window};
pub use context_transformer::{CompactionReport, ContextTransformer, SlidingWindowTransformer};
pub use async_context_transformer::AsyncContextTransformer;
pub use context_version::{
    ContextSummarizer, ContextVersion, ContextVersionMeta, ContextVersionStore,
    InMemoryVersionStore, VersioningTransformer,
};
pub use convert::{MessageConverter, ToolSchema, convert_messages, extract_tool_schemas};
```
