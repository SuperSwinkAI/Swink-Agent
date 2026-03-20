# Quickstart: Agent Struct & Public API

**Feature**: 005-agent-struct | **Date**: 2026-03-20

## Build & Test

```bash
# Build the workspace
cargo build --workspace

# Run all tests
cargo test --workspace

# Run agent-specific tests
cargo test -p swink-agent agent

# Run with no default features (verify builtin-tools disabled)
cargo test -p swink-agent --no-default-features

# Lint
cargo clippy --workspace -- -D warnings
```

## Usage Examples

### Basic prompt (async)

```rust
use swink_agent::{Agent, AgentOptions, ModelSpec, default_convert};

let model = ModelSpec::new("anthropic", "claude-sonnet-4-20250514");
let options = AgentOptions::new_simple("You are a helpful assistant.", model, my_stream_fn);
let mut agent = Agent::new(options);

let result = agent.prompt_text("What is 2 + 2?").await?;
println!("Response: {:?}", result.stop_reason);
```

### Basic prompt (sync / blocking)

```rust
let result = agent.prompt_text_sync("What is 2 + 2?")?;
```

### Streaming

```rust
use futures::StreamExt;

let mut stream = agent.prompt_stream(messages)?;
while let Some(event) = stream.next().await {
    agent.handle_stream_event(&event);
    match event {
        AgentEvent::ContentDelta { text, .. } => print!("{text}"),
        AgentEvent::AgentEnd { .. } => break,
        _ => {}
    }
}
```

### Subscribing to events

```rust
let sub_id = agent.subscribe(|event| {
    println!("Event: {event:?}");
});

agent.prompt_text("Hello").await?;

// Later, unsubscribe
agent.unsubscribe(sub_id);
```

### Steering mid-run

```rust
use swink_agent::types::{AgentMessage, LlmMessage, UserMessage, ContentBlock};

// While the agent is running (e.g., from another task):
agent.steer(AgentMessage::Llm(LlmMessage::User(UserMessage {
    content: vec![ContentBlock::Text {
        text: "Actually, focus on summarizing instead.".into(),
    }],
    timestamp: swink_agent::util::now_timestamp(),
})));
```

### Structured output

```rust
use serde_json::json;

let schema = json!({
    "type": "object",
    "properties": {
        "name": { "type": "string" },
        "age": { "type": "integer" }
    },
    "required": ["name", "age"]
});

let value = agent.structured_output(
    "Extract the person's info: John is 30 years old.".into(),
    schema,
).await?;

println!("Name: {}", value["name"]);
```

### Structured output with typed deserialization

```rust
#[derive(serde::Deserialize)]
struct Person {
    name: String,
    age: u32,
}

let person: Person = agent.structured_output_typed(
    "Extract: John is 30.".into(),
    schema,
).await?;
```

### State mutation between runs

```rust
agent.set_system_prompt("You are a code reviewer.");
agent.set_model(ModelSpec::new("anthropic", "claude-sonnet-4-20250514"));
agent.clear_messages();
```

### Continue from existing context

```rust
let result1 = agent.prompt_text("Tell me about Rust.").await?;
// Agent now has history. Continue the conversation:
agent.follow_up(user_message("Now compare it to Go."));
let result2 = agent.continue_async().await?;
```

### Abort and reset

```rust
agent.abort();  // Cancel current run
agent.wait_for_idle().await;  // Wait for it to stop
agent.reset();  // Clear all state
```

### Spawn as background task

```rust
use swink_agent::AgentHandle;

let handle = AgentHandle::spawn(agent, messages);
// ... do other work ...
let result = handle.await_result().await?;
```
