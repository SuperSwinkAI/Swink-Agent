# Quickstart: Foundation Types & Errors

**Feature**: 002-foundation-types-errors

## Prerequisites

- Feature 001 (workspace scaffold) complete
- Rust 1.88 toolchain

## Build & Test

```bash
cargo build -p swink-agent
cargo test -p swink-agent
cargo clippy -p swink-agent -- -D warnings
```

## Usage Examples

### Creating Messages

```rust
use swink_agent::{ContentBlock, LlmMessage, AgentMessage, Usage, Cost, StopReason};
use std::time::SystemTime;

// User message with text
let user_msg = LlmMessage::User {
    content: vec![ContentBlock::Text { text: "Hello!".into() }],
    timestamp: SystemTime::now(),
};

// Assistant message
let assistant_msg = LlmMessage::Assistant {
    content: vec![ContentBlock::Text { text: "Hi there!".into() }],
    provider: "anthropic".into(),
    model: "claude-sonnet-4-6".into(),
    usage: Usage::default(),
    cost: Cost::default(),
    stop_reason: StopReason::Stop,
    error_message: None,
    timestamp: SystemTime::now(),
};

// Wrap as AgentMessage
let agent_msg = AgentMessage::Llm(user_msg);
```

### Aggregating Usage

```rust
let usage1 = Usage { input_tokens: 100, output_tokens: 50, ..Default::default() };
let usage2 = Usage { input_tokens: 200, output_tokens: 75, ..Default::default() };
let total = usage1 + usage2;
assert_eq!(total.input_tokens, 300);
assert_eq!(total.output_tokens, 125);
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

struct MyNotification { text: String }

impl CustomMessage for MyNotification {
    fn as_any(&self) -> &dyn Any { self }
    fn type_name(&self) -> &str { "MyNotification" }
}

let msg = AgentMessage::Custom(Box::new(MyNotification { text: "done".into() }));

// Downcast back
match msg.downcast_ref::<MyNotification>() {
    Ok(notif) => println!("Got: {}", notif.text),
    Err(e) => eprintln!("Wrong type: {e}"),
}
```

## Verification Checklist

- [ ] `cargo build -p swink-agent` compiles with zero errors
- [ ] `cargo test -p swink-agent` passes all type tests
- [ ] `cargo clippy -p swink-agent -- -D warnings` reports zero warnings
- [ ] All public types accessible via `use swink_agent::*`
- [ ] Serialization round-trips produce identical output
- [ ] Usage/Cost aggregation is arithmetically correct
