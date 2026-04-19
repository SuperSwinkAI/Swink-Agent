# Quickstart: Context Management

**Feature**: 006-context-management | **Date**: 2026-03-20 | **Updated**: 2026-03-31

## Build & Test

```bash
# Build the workspace
cargo build --workspace

# Run all tests
cargo test --workspace

# Run context-specific tests
cargo test -p swink-agent context
cargo test -p swink-agent convert

# Run with no default features (verify builtin-tools disabled)
cargo test -p swink-agent --no-default-features

# Lint
cargo clippy --workspace -- -D warnings
```

## Usage Examples

### Sliding window with default token estimation

```rust
use swink_agent::sliding_window;

// Create a sliding window: 100k normal budget, 50k overflow budget, 2 anchor messages
let compact = sliding_window(100_000, 50_000, 2);

// Apply to a mutable message list (overflow = false for normal operation)
compact(&mut messages, false);

// After context overflow, use overflow budget for more aggressive pruning
compact(&mut messages, true);
```

### SlidingWindowTransformer with compaction reporting

```rust
use swink_agent::{SlidingWindowTransformer, ContextTransformer};

let transformer = SlidingWindowTransformer::new(100_000, 50_000, 2);

if let Some(report) = transformer.transform(&mut messages, false) {
    println!(
        "Dropped {} messages ({} -> {} tokens)",
        report.dropped_count, report.tokens_before, report.tokens_after
    );
}
```

### Custom token counter

```rust
use swink_agent::{SlidingWindowTransformer, TiktokenCounter};
use std::sync::Arc;

let transformer = SlidingWindowTransformer::new(100_000, 50_000, 2)
    .with_token_counter(Arc::new(
        TiktokenCounter::cl100k().expect("built-in cl100k tokenizer"),
    ));
```

Enable the `tiktoken` feature to use the built-in wrapper:

```bash
cargo add swink-agent --features tiktoken
```

### Custom synchronous context transformer

```rust
use swink_agent::{ContextTransformer, CompactionReport};
use swink_agent::types::AgentMessage;

struct RagInjector {
    knowledge_base: Vec<String>,
}

impl ContextTransformer for RagInjector {
    fn transform(
        &self,
        messages: &mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Option<CompactionReport> {
        // Inject relevant context at position 1 (after first anchor)
        // ... retrieval logic ...
        None // No compaction, just injection
    }
}
```

### Async context transformer

```rust
use swink_agent::{AsyncContextTransformer, CompactionReport};
use swink_agent::types::AgentMessage;
use std::future::Future;
use std::pin::Pin;

struct AsyncSummarizer;

impl AsyncContextTransformer for AsyncSummarizer {
    fn transform<'a>(
        &'a self,
        messages: &'a mut Vec<AgentMessage>,
        overflow: bool,
    ) -> Pin<Box<dyn Future<Output = Option<CompactionReport>> + Send + 'a>> {
        Box::pin(async move {
            // Fetch summary from an LLM or database
            // Inject as a message
            None
        })
    }
}
```

### Context versioning with compaction capture

```rust
use swink_agent::{
    SlidingWindowTransformer, VersioningTransformer,
    InMemoryVersionStore,
};
use std::sync::Arc;

let store = Arc::new(InMemoryVersionStore::new());
let inner = SlidingWindowTransformer::new(100_000, 50_000, 2);
let transformer = VersioningTransformer::new(inner, Arc::clone(&store));

// After compaction, dropped messages are captured as versions
transformer.transform(&mut messages, false);

// Retrieve version history
for meta in store.list_versions() {
    println!("Version {}: {} messages (turn {})", meta.version, meta.message_count, meta.turn);
}

// Load a specific version
if let Some(v) = store.load_version(1) {
    println!("Dropped {} messages", v.messages.len());
}
```

### Configuring context management on AgentOptions

```rust
use swink_agent::{AgentOptions, SlidingWindowTransformer, VersioningTransformer, InMemoryVersionStore};
use std::sync::Arc;

// Option 1: Simple closure-based transform
let options = AgentOptions::new_simple("prompt", model, stream_fn)
    .with_transform_context_fn(|messages, overflow| {
        if overflow && messages.len() > 5 {
            messages.truncate(5);
        }
    });

// Option 2: Struct-based transform with reporting
let options = AgentOptions::new_simple("prompt", model, stream_fn)
    .with_transform_context(SlidingWindowTransformer::new(100_000, 50_000, 2));

// Option 3: Versioning wrapper
let store = Arc::new(InMemoryVersionStore::new());
let transformer = VersioningTransformer::new(
    SlidingWindowTransformer::new(100_000, 50_000, 2),
    store,
);
let options = AgentOptions::new_simple("prompt", model, stream_fn)
    .with_transform_context(transformer);
```

### Context caching with static/dynamic prompt split

```rust
use swink_agent::{AgentOptions, CacheConfig};
use std::time::Duration;

// Configure caching: 10-minute TTL, minimum 4096 tokens, refresh every 3 turns
let cache_config = CacheConfig {
    ttl: Duration::from_secs(600),
    min_tokens: 4096,
    cache_intervals: 3,
};

let options = AgentOptions::new_simple("default prompt", model, stream_fn)
    .with_static_system_prompt("You are a helpful assistant with deep expertise in...".into())
    .with_dynamic_system_prompt(|| {
        format!("Current time: {}. User timezone: UTC-5.", chrono::Utc::now())
    })
    .with_cache_config(cache_config);

// Turn 1: static prompt sent with CacheHint::Write { ttl: 600s }
// Turns 2-3: static prompt sent with CacheHint::Read
// Turn 4: cache refreshed with new CacheHint::Write
```

### Pre-flight context overflow check

```rust
use swink_agent::{is_context_overflow, estimate_tokens};
use swink_agent::types::ModelSpec;

// Check before sending
if is_context_overflow(&messages, &model, None) {
    // Trigger compaction or warn the user
    transformer.transform(&mut messages, true); // overflow=true for aggressive pruning
}

// With a custom token counter
if is_context_overflow(&messages, &model, Some(&my_tiktoken_counter)) {
    // Handle overflow
}
```

### Message conversion (adapter implementation)

```rust
use swink_agent::convert::{MessageConverter, convert_messages};
use swink_agent::types::{UserMessage, AssistantMessage, ToolResultMessage};

struct MyProviderMessage { /* ... */ }

struct MyConverter;

impl MessageConverter for MyConverter {
    type Message = MyProviderMessage;

    fn system_message(prompt: &str) -> Option<Self::Message> {
        Some(MyProviderMessage { /* ... */ })
    }

    fn user_message(user: &UserMessage) -> Self::Message {
        MyProviderMessage { /* ... */ }
    }

    fn assistant_message(assistant: &AssistantMessage) -> Self::Message {
        MyProviderMessage { /* ... */ }
    }

    fn tool_result_message(result: &ToolResultMessage) -> Self::Message {
        MyProviderMessage { /* ... */ }
    }
}

// Convert agent messages to provider format (CustomMessage variants are skipped)
let provider_messages = convert_messages::<MyConverter>(&agent_messages, "system prompt");
```
