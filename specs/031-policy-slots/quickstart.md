# Quickstart: Configurable Policy Slots

## No policies (default — anything goes)

```rust
let agent = Agent::new(AgentOptions::new(
    system_prompt,
    model,
    stream_fn,
    convert_to_llm,
));
// Loop runs with no restrictions. Same as before.
```

## Add budget enforcement

```rust
use swink_agent::{BudgetPolicy, Agent, AgentOptions};

let agent = Agent::new(
    AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm)
        .with_pre_turn_policy(BudgetPolicy::new().max_cost(5.0))
);
// Loop stops gracefully when accumulated cost reaches $5.00.
```

## Stack multiple policies

```rust
use swink_agent::{BudgetPolicy, MaxTurnsPolicy, Agent, AgentOptions};

let agent = Agent::new(
    AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm)
        .with_pre_turn_policy(BudgetPolicy::new().max_cost(10.0))
        .with_pre_turn_policy(MaxTurnsPolicy::new(20))
);
// Stops on whichever limit is hit first.
// Budget is checked first (vec order = priority).
```

## Block specific tools

```rust
use swink_agent::{ToolDenyListPolicy, Agent, AgentOptions};

let agent = Agent::new(
    AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm)
        .with_pre_dispatch_policy(ToolDenyListPolicy::new(["bash", "write_file"]))
);
// "bash" and "write_file" calls are skipped with error text sent to the LLM.
// Other tools work normally.
```

## Sandbox file paths

```rust
use swink_agent::{SandboxPolicy, Agent, AgentOptions};

let agent = Agent::new(
    AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm)
        .with_pre_dispatch_policy(SandboxPolicy::new("/tmp/workspace"))
);
// File operations outside /tmp/workspace are rejected.
```

## Detect stuck loops

```rust
use swink_agent::{LoopDetectionPolicy, Agent, AgentOptions};

let agent = Agent::new(
    AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm)
        .with_post_turn_policy(
            LoopDetectionPolicy::new(3)
                .with_steering("You appear to be repeating the same action. Try a different approach.")
        )
);
// If the same tool+args pattern repeats 3 turns in a row,
// a steering message is injected to redirect the model.
```

## Checkpoint after every turn

```rust
use swink_agent::{CheckpointPolicy, Agent, AgentOptions};

let agent = Agent::new(
    AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm)
        .with_post_turn_policy(CheckpointPolicy::new(checkpoint_store))
);
// State is persisted after each turn for crash recovery.
```

## Custom policy

```rust
use swink_agent::{PreTurnPolicy, PolicyContext, PolicyVerdict};

struct RateLimitPolicy {
    calls: AtomicU64,
    max_calls: u64,
}

impl PreTurnPolicy for RateLimitPolicy {
    fn name(&self) -> &str { "rate_limit" }

    fn evaluate(&self, _ctx: &PolicyContext<'_>) -> PolicyVerdict {
        let count = self.calls.fetch_add(1, Ordering::Relaxed);
        if count >= self.max_calls {
            PolicyVerdict::Stop(format!("Rate limit exceeded: {} calls", count))
        } else {
            PolicyVerdict::Continue
        }
    }
}

let agent = Agent::new(
    AgentOptions::new(system_prompt, model, stream_fn, convert_to_llm)
        .with_pre_turn_policy(RateLimitPolicy { calls: AtomicU64::new(0), max_calls: 100 })
);
```
