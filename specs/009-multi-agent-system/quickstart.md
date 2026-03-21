# Quickstart: Multi-Agent System

**Feature**: 009-multi-agent-system | **Date**: 2026-03-20

## Build & Test

```bash
# Build the workspace
cargo build --workspace

# Run all tests
cargo test --workspace

# Run orchestrator tests specifically
cargo test -p swink-agent orchestrator

# Lint
cargo clippy --workspace -- -D warnings
```

## Usage Examples

### Register and look up agents

```rust
use swink_agent::{Agent, AgentOptions, AgentRegistry, ModelSpec};

let registry = AgentRegistry::new();

let options = AgentOptions::new_simple("You are a planner.", model, stream_fn);
let agent = Agent::new(options);

// Register returns an AgentRef (Arc<tokio::sync::Mutex<Agent>>)
let agent_ref = registry.register("planner", agent);

// Look up by name from any thread
let found = registry.get("planner").expect("agent exists");
let agent = found.lock().await;
println!("Agent ID: {}", agent.id());
```

### Send messages between agents

```rust
use swink_agent::{AgentRegistry, messaging::send_to};
use swink_agent::types::{AgentMessage, LlmMessage, UserMessage, ContentBlock};
use swink_agent::util::now_timestamp;

let registry = AgentRegistry::new();
// ... register agents ...

let message = AgentMessage::Llm(LlmMessage::User(UserMessage {
    content: vec![ContentBlock::Text { text: "Analyze this data.".into() }],
    timestamp: now_timestamp(),
}));

// Send a message to the "analyst" agent via its steering queue
send_to(&registry, "analyst", message).await?;
```

### Use AgentMailbox as a standalone inbox

```rust
use swink_agent::AgentMailbox;
use swink_agent::types::{AgentMessage, LlmMessage, UserMessage, ContentBlock};
use swink_agent::util::now_timestamp;

let mailbox = AgentMailbox::new();

// Send from one task
mailbox.send(AgentMessage::Llm(LlmMessage::User(UserMessage {
    content: vec![ContentBlock::Text { text: "Hello".into() }],
    timestamp: now_timestamp(),
})));

// Drain from another task
let pending = mailbox.drain();
assert_eq!(pending.len(), 1);
```

### Invoke an agent as a tool (SubAgent)

```rust
use swink_agent::{Agent, AgentOptions, SubAgent, ModelSpec};
use std::sync::Arc;

// Create a sub-agent tool
let researcher = SubAgent::simple(
    "researcher",
    "Research Agent",
    "Researches a topic and returns findings.",
    "You are a research assistant. Provide thorough analysis.",
    ModelSpec::new("anthropic", "claude-sonnet-4-20250514"),
    Arc::clone(&stream_fn),
);

// Add it as a tool to a parent agent
let parent_options = AgentOptions::new_simple(
    "You are a project manager. Use the researcher tool for deep dives.",
    model,
    stream_fn,
).with_tools(vec![Arc::new(researcher)]);

let mut parent = Agent::new(parent_options);
let result = parent.prompt_text("Research quantum computing trends.").await?;
```

### SubAgent with custom options and result mapping

```rust
use swink_agent::{SubAgent, AgentOptions, AgentToolResult};

let custom_agent = SubAgent::new("custom", "Custom Agent", "A customized sub-agent")
    .with_options(move || {
        AgentOptions::new_simple("Custom prompt.", model.clone(), Arc::clone(&stream_fn))
            .with_tools(vec![/* child-specific tools */])
    })
    .with_map_result(|result| {
        // Custom result mapping
        AgentToolResult::text(format!("Processed: {:?}", result.stop_reason))
    })
    .with_requires_approval(true);
```

### Orchestrate multiple agents with supervision

```rust
use swink_agent::{AgentOrchestrator, AgentOptions, ModelSpec, DefaultSupervisor};

let mut orchestrator = AgentOrchestrator::new()
    .with_supervisor(DefaultSupervisor::new(3))
    .with_channel_buffer(64);

// Register a parent agent
orchestrator.add_agent("planner", || {
    AgentOptions::new_simple("You plan tasks.", model.clone(), Arc::clone(&stream_fn))
});

// Register child agents under the parent
orchestrator.add_child("researcher", "planner", || {
    AgentOptions::new_simple("You research topics.", model.clone(), Arc::clone(&stream_fn))
});

orchestrator.add_child("writer", "planner", || {
    AgentOptions::new_simple("You write content.", model.clone(), Arc::clone(&stream_fn))
});

// Spawn and interact
let handle = orchestrator.spawn("planner")?;
let result = handle.send_message("Plan a blog post about Rust async.").await?;

// Check status
assert!(!handle.is_done());

// Cancel when done
handle.cancel();
```

### Await final result from orchestrated agent

```rust
let handle = orchestrator.spawn("researcher")?;
let result = handle.send_message("What is quantum entanglement?").await?;

// Consume the handle and await shutdown
let final_result = handle.await_result().await?;
```
