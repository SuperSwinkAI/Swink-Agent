# Quickstart: Foundation Types & Errors

**Feature**: 002-foundation-types-errors

## Prerequisites

- Feature 001 (workspace scaffold) complete
- Rust latest stable toolchain

## Build & Test

```bash
cargo build -p swink-agent
cargo test -p swink-agent
cargo clippy -p swink-agent -- -D warnings
```

## Usage Examples

### Creating Messages

```rust
use swink_agent::{
    ContentBlock, LlmMessage, AgentMessage, UserMessage, AssistantMessage,
    Usage, Cost, StopReason,
};

// User message with text
let user_msg = LlmMessage::User(UserMessage {
    content: vec![ContentBlock::Text { text: "Hello!".into() }],
    timestamp: 0,
});

// Assistant message
let assistant_msg = LlmMessage::Assistant(AssistantMessage {
    content: vec![ContentBlock::Text { text: "Hi there!".into() }],
    provider: "anthropic".into(),
    model_id: "claude-sonnet-4-6".into(),
    usage: Usage::default(),
    cost: Cost::default(),
    stop_reason: StopReason::Stop,
    error_message: None,
    timestamp: 0,
});

// Wrap as AgentMessage
let agent_msg = AgentMessage::Llm(user_msg);
```

### Aggregating Usage

```rust
use swink_agent::Usage;

let usage1 = Usage { input: 100, output: 50, ..Default::default() };
let usage2 = Usage { input: 200, output: 75, ..Default::default() };
let total = usage1 + usage2;
assert_eq!(total.input, 300);
assert_eq!(total.output, 125);
```

### Handling Errors

```rust
use swink_agent::AgentError;

fn handle_error(err: AgentError) {
    match err {
        AgentError::ContextWindowOverflow { model } => {
            eprintln!("Context too large for {model}, pruning...");
        }
        AgentError::ModelThrottled => {
            eprintln!("Rate limited, will retry...");
        }
        AgentError::Aborted => {
            eprintln!("Run cancelled.");
        }
        other => eprintln!("Error: {other}"),
    }
}
```

### Custom Messages

```rust
use swink_agent::{AgentMessage, CustomMessage};
use std::any::Any;
use std::fmt;

#[derive(Debug)]
struct MyNotification { text: String }

impl CustomMessage for MyNotification {
    fn as_any(&self) -> &dyn Any { self }
    fn type_name(&self) -> Option<&str> { Some("MyNotification") }
}

let msg = AgentMessage::Custom(Box::new(MyNotification { text: "done".into() }));

// Downcast back
match msg.downcast_ref::<MyNotification>() {
    Ok(notif) => println!("Got: {}", notif.text),
    Err(e) => eprintln!("Wrong type: {e}"),
}
```

## Verification Checklist

- [x] `cargo build -p swink-agent` compiles with zero errors
- [x] `cargo test -p swink-agent` passes all type tests
- [x] `cargo clippy -p swink-agent -- -D warnings` reports zero warnings
- [x] All public types accessible via `use swink_agent::*`
- [x] Serialization round-trips produce identical output
- [x] Usage/Cost aggregation is arithmetically correct
