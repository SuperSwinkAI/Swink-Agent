# swink-agent-policies

[![Crates.io](https://img.shields.io/crates/v/swink-agent-policies.svg)](https://crates.io/crates/swink-agent-policies)
[![Docs.rs](https://docs.rs/swink-agent-policies/badge.svg)](https://docs.rs/swink-agent-policies)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

Ready-made policy implementations for [`swink-agent`](https://crates.io/crates/swink-agent) — budget caps, PII redaction, prompt-injection guard, audit logging, and more, each behind its own feature flag.

## Features

**Core policies:**
- **`budget`** — `BudgetPolicy` stops the loop when cost or token limits are exceeded
- **`max-turns`** — `MaxTurnsPolicy` caps a run at N turns
- **`deny-list`** — `ToolDenyListPolicy` rejects named tools before dispatch
- **`sandbox`** — `SandboxPolicy` restricts filesystem tools to a root directory
- **`loop-detection`** — `LoopDetectionPolicy` spots repeated tool-call patterns
- **`checkpoint`** — `CheckpointPolicy` persists agent state after each turn

**Application policies:**
- **`prompt-guard`** — `PromptInjectionGuard` blocks suspicious patterns in user messages and tool results
- **`pii`** — `PiiRedactor` scrubs emails/phones/SSNs from assistant responses
- **`content-filter`** — keyword/regex blocklist for assistant output
- **`audit`** — `AuditLogger` records every turn to a pluggable sink (JSONL, Unix socket, HTTP, etc.)

All policies slot into the four core hook points (`pre_turn`, `pre_dispatch`, `post_turn`, `post_loop`) and are evaluated in insertion order.

## Quick Start

```toml
[dependencies]
swink-agent = "0.8"
swink-agent-policies = { version = "0.8", features = ["budget", "max-turns", "deny-list"] }
tokio = { version = "1", features = ["full"] }
```

```rust,ignore
use swink_agent::prelude::*;
use swink_agent_policies::{BudgetPolicy, MaxTurnsPolicy, ToolDenyListPolicy};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = AgentOptions::from_connections("You are a helpful assistant.", connections)
        .with_pre_turn_policy(BudgetPolicy::new().max_cost(10.0))
        .with_pre_turn_policy(MaxTurnsPolicy::new(5))
        .with_pre_dispatch_policy(ToolDenyListPolicy::new(["bash"]));

    let mut agent = Agent::new(options);
    let result = agent.prompt_text("Hello!").await?;
    println!("{}", result.assistant_text());
    Ok(())
}
```

See `cargo run -p swink-agent-policies --example with_policies` for a runnable end-to-end demo.

## Architecture

Every policy implements one of the four `Policy*` traits exposed by the core crate. When a slot fires, the loop iterates policies in registration order; the first `PolicyDecision::Stop` short-circuits the turn. Policies never mutate agent state directly — they return structured decisions the core loop acts on, which keeps their behavior testable in isolation and composable without ordering surprises beyond the declared slot.

No `unsafe` code (`#![forbid(unsafe_code)]`). Application policies (`pii`, `prompt-guard`, `content-filter`) use compiled regexes with bounded memory; patterns are validated at policy construction time, not per-call.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
