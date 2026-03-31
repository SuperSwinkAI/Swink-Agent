# Quickstart: Agent Loop

**Feature**: 004-agent-loop

## Prerequisites

- Features 001-003 complete (workspace, types, traits)
- Rust 1.88 toolchain

## Build & Test

```bash
cargo build -p swink-agent
cargo test -p swink-agent
cargo clippy -p swink-agent -- -D warnings
```

## Usage Examples

### Simple Single-Turn Loop

```rust
use swink_agent::{
    agent_loop, AgentContext, AgentLoopConfig, AgentMessage,
    ContentBlock, LlmMessage, ModelSpec, StreamOptions,
};
use tokio_util::sync::CancellationToken;
use futures::StreamExt;

// Set up context with a user message
let context = AgentContext {
    system_prompt: "You are helpful.".into(),
    messages: vec![AgentMessage::Llm(LlmMessage::User {
        content: vec![ContentBlock::Text { text: "Hello!".into() }],
        timestamp: SystemTime::now(),
    })],
    tools: vec![],
};

// Configure the loop
let config = AgentLoopConfig {
    model: ModelSpec { provider: "mock".into(), model_id: "test".into(), .. },
    stream_options: StreamOptions::default(),
    retry_strategy: Box::new(DefaultRetryStrategy::default()),
    convert_to_llm: Arc::new(|msg| match msg {
        AgentMessage::Llm(llm) => Some(llm.clone()),
        AgentMessage::Custom(_) => None,
    }),
    transform_context: None, // identity — no pruning
    get_api_key: None,
    get_steering_messages: None,
    get_follow_up_messages: None,
};

// Run the loop
let token = CancellationToken::new();
let mut stream = agent_loop(vec![], context, config, token);

while let Some(event) = stream.next().await {
    match event {
        AgentEvent::MessageUpdate { delta } => { /* handle streaming delta */ }
        AgentEvent::AgentEnd { messages } => { /* done */ }
        _ => {}
    }
}
```

### Cancellation

```rust
let token = CancellationToken::new();
let token_clone = token.clone();

// Cancel after 5 seconds
tokio::spawn(async move {
    tokio::time::sleep(Duration::from_secs(5)).await;
    token_clone.cancel();
});

let mut stream = agent_loop(vec![], context, config, token);
// Stream will emit AgentEnd with StopReason::Aborted when cancelled
```

## Verification Checklist

- [ ] Single-turn conversation emits events in correct lifecycle order
- [ ] Tool calls execute concurrently (not sequentially)
- [ ] Steering interrupts cancel remaining tools
- [ ] Follow-up messages cause loop continuation
- [ ] Error/abort exits skip follow-up polling
- [ ] Retry strategy consulted for transient failures
- [ ] Context overflow triggers emergency recovery (compact + retry), second failure surfaces error
- [ ] Max tokens recovery replaces incomplete tool calls
- [ ] Cancellation produces clean aborted shutdown
