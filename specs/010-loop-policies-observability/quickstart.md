# Quickstart: Loop Policies & Observability

**Feature**: 010-loop-policies-observability | **Date**: 2026-03-20

## Build & Test

```bash
# Build the workspace
cargo build --workspace

# Run all tests
cargo test --workspace

# Run tests for specific modules
cargo test -p swink-agent loop_policy
cargo test -p swink-agent stream_middleware
cargo test -p swink-agent metrics
cargo test -p swink-agent post_turn_hook
cargo test -p swink-agent budget_guard
cargo test -p swink-agent checkpoint

# Lint
cargo clippy --workspace -- -D warnings
```

## Usage Examples

### Limit agent turns with a policy

```rust
use swink_agent::{Agent, AgentOptions, MaxTurnsPolicy};

let options = AgentOptions::new_simple("You are helpful.", model, stream_fn)
    .with_loop_policy(MaxTurnsPolicy::new(10));

let mut agent = Agent::new(options);
let result = agent.prompt_text("Hello").await?;
// Agent will stop after at most 10 turns
```

### Cap cost with a policy

```rust
use swink_agent::CostCapPolicy;

let options = AgentOptions::new_simple("You are helpful.", model, stream_fn)
    .with_loop_policy(CostCapPolicy::new(5.0)); // $5 max
```

### Compose multiple policies

```rust
use swink_agent::{ComposedPolicy, MaxTurnsPolicy, CostCapPolicy};

let policy = ComposedPolicy::new(vec![
    Box::new(MaxTurnsPolicy::new(20)),
    Box::new(CostCapPolicy::new(10.0)),
]);
// Stops when EITHER limit is hit
```

### Use a closure as a policy

```rust
let policy = |ctx: &swink_agent::PolicyContext<'_>| {
    ctx.turn_index < 5 && ctx.accumulated_cost.total < 1.0
};
```

### Wrap the stream with logging middleware

```rust
use std::sync::Arc;
use swink_agent::StreamMiddleware;

let logged = StreamMiddleware::with_logging(stream_fn, |event| {
    println!("event: {event:?}");
});

let options = AgentOptions::new_simple("prompt", model, Arc::new(logged));
```

### Filter events from the stream

```rust
use swink_agent::{StreamMiddleware, AssistantMessageEvent};

let filtered = StreamMiddleware::with_filter(stream_fn, |event| {
    !matches!(event,
        AssistantMessageEvent::ThinkingStart { .. }
        | AssistantMessageEvent::ThinkingDelta { .. }
        | AssistantMessageEvent::ThinkingEnd { .. }
    )
});
```

### Chain multiple middleware layers

```rust
use std::sync::Arc;
use swink_agent::{StreamMiddleware, stream::StreamFn};

// Layer 1: log
let logged: Arc<dyn StreamFn> = Arc::new(
    StreamMiddleware::with_logging(stream_fn, |event| {
        tracing::debug!(?event, "stream event");
    })
);

// Layer 2: transform
let transformed = StreamMiddleware::with_map(logged, |event| {
    // modify events as needed
    event
});
```

### Collect metrics

```rust
use std::future::Future;
use std::pin::Pin;
use swink_agent::metrics::{MetricsCollector, TurnMetrics};

struct LogMetrics;

impl MetricsCollector for LogMetrics {
    fn on_metrics<'a>(
        &'a self,
        metrics: &'a TurnMetrics,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            println!(
                "Turn {}: LLM {:?}, {} tools, {} tokens, ${:.4}",
                metrics.turn_index,
                metrics.llm_call_duration,
                metrics.tool_executions.len(),
                metrics.usage.total,
                metrics.cost.total,
            );
        })
    }
}
```

### Post-turn hook that stops on cost

```rust
use swink_agent::post_turn_hook::{PostTurnHook, PostTurnContext, PostTurnAction};
use std::future::Future;
use std::pin::Pin;

struct CostLimitHook { max_cost: f64 }

impl PostTurnHook for CostLimitHook {
    fn on_turn_end<'a>(
        &'a self,
        ctx: &'a PostTurnContext<'a>,
    ) -> Pin<Box<dyn Future<Output = PostTurnAction> + Send + 'a>> {
        Box::pin(async move {
            if ctx.accumulated_cost.total > self.max_cost {
                PostTurnAction::Stop(Some("cost limit exceeded".into()))
            } else {
                PostTurnAction::Continue
            }
        })
    }
}
```

### Post-turn hook that injects messages

```rust
use swink_agent::post_turn_hook::{PostTurnHook, PostTurnContext, PostTurnAction};
use swink_agent::types::{AgentMessage, LlmMessage, UserMessage, ContentBlock};

struct InjectFollowUp;

impl PostTurnHook for InjectFollowUp {
    fn on_turn_end<'a>(
        &'a self,
        ctx: &'a PostTurnContext<'a>,
    ) -> Pin<Box<dyn std::future::Future<Output = PostTurnAction> + Send + 'a>> {
        Box::pin(async move {
            if ctx.turn_index == 0 {
                PostTurnAction::InjectMessages(vec![
                    AgentMessage::Llm(LlmMessage::User(UserMessage {
                        content: vec![ContentBlock::Text {
                            text: "Please elaborate.".into(),
                        }],
                        timestamp: 0,
                    }))
                ])
            } else {
                PostTurnAction::Continue
            }
        })
    }
}
```

### Set a budget guard

```rust
use swink_agent::BudgetGuard;

// Cost-only guard
let guard = BudgetGuard::new().with_max_cost(5.0);

// Token-only guard
let guard = BudgetGuard::new().with_max_tokens(100_000);

// Both limits
let guard = BudgetGuard::new()
    .with_max_cost(5.0)
    .with_max_tokens(100_000);

// Check before an LLM call
match guard.check(&accumulated_usage, &accumulated_cost) {
    Ok(()) => { /* proceed with LLM call */ }
    Err(exceeded) => {
        eprintln!("Budget exceeded: {exceeded}");
        // Cancel the agent
    }
}
```

### Save and restore checkpoints

```rust
use swink_agent::checkpoint::{Checkpoint, CheckpointStore};

// Create a checkpoint from agent state
let checkpoint = Checkpoint::new(
    "cp-001",
    "Be helpful.",
    "anthropic",
    "claude-sonnet-4-20250514",
    &agent_messages,
)
.with_turn_count(5)
.with_usage(accumulated_usage)
.with_cost(accumulated_cost)
.with_metadata("session_id", serde_json::json!("sess-abc"));

// Save via a CheckpointStore implementation
store.save_checkpoint(&checkpoint).await?;

// Restore later
if let Some(loaded) = store.load_checkpoint("cp-001").await? {
    let messages = loaded.restore_messages();
    // Resume agent with restored messages
}
```

### Use LoopCheckpoint for pause/resume

```rust
use swink_agent::checkpoint::LoopCheckpoint;

// Create from current loop state
let loop_cp = LoopCheckpoint::new("prompt", "anthropic", "claude", &messages)
    .with_turn_index(3)
    .with_usage(usage)
    .with_cost(cost)
    .with_overflow_signal(false)
    .with_last_assistant_message(last_msg);

// Convert to a standard Checkpoint for storage
let checkpoint = loop_cp.to_checkpoint("cp-loop-001");
store.save_checkpoint(&checkpoint).await?;

// Restore pending messages
let pending = loop_cp.restore_pending_messages();
```
