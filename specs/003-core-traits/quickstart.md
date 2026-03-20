# Quickstart: Core Traits

**Feature**: 003-core-traits

## Prerequisites

- Features 001-002 complete (workspace, types)
- Rust 1.88 toolchain

## Build & Test

```bash
cargo build -p swink-agent
cargo test -p swink-agent
cargo clippy -p swink-agent -- -D warnings
```

## Usage Examples

### Implementing a Custom Tool

```rust
use swink_agent::{AgentTool, AgentToolResult, ContentBlock};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

struct GreetTool;

impl AgentTool for GreetTool {
    fn name(&self) -> &str { "greet" }
    fn label(&self) -> &str { "Greet" }
    fn description(&self) -> &str { "Greets a person by name" }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        })
    }
    async fn execute(
        &self, _call_id: &str, args: Value,
        _token: CancellationToken, _cb: Option<Box<dyn Fn(String) + Send>>,
    ) -> AgentToolResult {
        let name = args["name"].as_str().unwrap_or("world");
        AgentToolResult::text(format!("Hello, {name}!"))
    }
}
```

### Implementing a Mock StreamFn

```rust
use swink_agent::{StreamFn, AssistantMessageEvent, Usage, Cost, StopReason};

struct MockStream;

impl StreamFn for MockStream {
    async fn call(&self, model: &ModelSpec, ctx: &AgentContext,
                  opts: &StreamOptions, token: CancellationToken)
        -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send>>
    {
        let events = vec![
            AssistantMessageEvent::Start { provider: "mock".into(), model: "test".into() },
            AssistantMessageEvent::TextStart { index: 0 },
            AssistantMessageEvent::TextDelta { index: 0, text: "Hello!".into() },
            AssistantMessageEvent::TextEnd { index: 0 },
            AssistantMessageEvent::Done { usage: Usage::default(), cost: Cost::default(), stop_reason: StopReason::Stop },
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
assert!(!strategy.should_retry(&AgentError::ModelThrottled, 4)); // exceeds max_attempts
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
