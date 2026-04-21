# Quickstart: Core Traits

**Feature**: 003-core-traits

## Prerequisites

- Features 001-002 complete (workspace, types)
- Rust latest stable toolchain

## Build & Test

```bash
cargo build -p swink-agent
cargo test -p swink-agent
cargo clippy -p swink-agent -- -D warnings
```

## Usage Examples

### Implementing a Custom Tool

```rust
use std::future::Future;
use std::pin::Pin;
use swink_agent::{AgentTool, AgentToolResult};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

struct GreetTool {
    schema: Value,
}

impl GreetTool {
    fn new() -> Self {
        Self {
            schema: json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
        }
    }
}

impl AgentTool for GreetTool {
    fn name(&self) -> &str { "greet" }
    fn label(&self) -> &str { "Greet" }
    fn description(&self) -> &str { "Greets a person by name" }
    fn parameters_schema(&self) -> &Value { &self.schema }
    fn execute(
        &self, _call_id: &str, args: Value,
        _token: CancellationToken,
        _on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async move {
            let name = args["name"].as_str().unwrap_or("world");
            AgentToolResult::text(format!("Hello, {name}!"))
        })
    }
}
```

### Implementing a Mock StreamFn

```rust
use std::pin::Pin;
use futures::Stream;
use swink_agent::{
    StreamFn, AssistantMessageEvent, StreamOptions,
    Usage, Cost, StopReason, ModelSpec, AgentContext,
};
use tokio_util::sync::CancellationToken;

struct MockStream;

impl StreamFn for MockStream {
    fn stream<'a>(
        &'a self, _model: &'a ModelSpec, _ctx: &'a AgentContext,
        _opts: &'a StreamOptions, _token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        let events = vec![
            AssistantMessageEvent::Start,
            AssistantMessageEvent::TextStart { content_index: 0 },
            AssistantMessageEvent::TextDelta { content_index: 0, delta: "Hello!".into() },
            AssistantMessageEvent::TextEnd { content_index: 0 },
            AssistantMessageEvent::Done {
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                cost: Cost::default(),
            },
        ];
        Box::pin(futures::stream::iter(events))
    }
}
```

### Using the Default Retry Strategy

```rust
use swink_agent::{DefaultRetryStrategy, RetryStrategy, AgentError};

let strategy = DefaultRetryStrategy::default();
assert!(strategy.should_retry(&AgentError::ModelThrottled, 1));
assert!(!strategy.should_retry(&AgentError::Aborted, 1));
assert!(!strategy.should_retry(&AgentError::ModelThrottled, 3)); // attempt == max_attempts
```

## Verification Checklist

- [ ] Mock tool validates arguments against schema
- [ ] Invalid arguments rejected with field-level errors
- [ ] Mock stream accumulates into finalized AssistantMessage
- [ ] Out-of-order events produce errors
- [ ] Empty stream produces error
- [ ] Default retry retries only ModelThrottled and NetworkError
- [ ] Exponential delays increase correctly and cap at max_delay
- [ ] Jitter varies delays within [0.5, 1.5) range
