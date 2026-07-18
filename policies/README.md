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
- **`checkpoint`** — `CheckpointPolicy` persists agent state after each turn; `RollingCheckpointPolicy` overwrites a single checkpoint for long-session crash-safety
- **`recommended`** — `RecommendedPolicies` preset bundles the four production guardrails (budget, max-turns, sandbox, deny-list) in one call, plus contract-test helpers

**Application policies:**
- **`prompt-guard`** — `PromptInjectionGuard` blocks suspicious patterns in user messages and tool results
- **`pii`** — `PiiRedactor` scrubs emails/phones/SSNs from assistant responses
- **`content-filter`** — keyword/regex blocklist for assistant output
- **`audit`** — `AuditLogger` records every turn to a pluggable sink (JSONL, Unix socket, HTTP, etc.)
- **`memory-nudge`** — `MemoryNudgePolicy` flags save-worthy content (corrections, decisions, preferences, explicit save requests) as injected extension blocks

All policies slot into the four core hook points (`pre_turn`, `pre_dispatch`, `post_turn`, `post_loop`) and are evaluated in insertion order.

## Quick Start

```toml
[dependencies]
swink-agent = "0.9.0"
swink-agent-policies = { version = "0.9.0", features = ["budget", "max-turns", "deny-list"] }
tokio = { version = "1", features = ["full"] }
```

```rust,ignore
use swink_agent::prelude::*;
use swink_agent_policies::{BudgetPolicy, MaxTurnsPolicy, ToolDenyListPolicy};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = AgentOptions::from_connections("You are a helpful assistant.", connections)
        .with_pre_turn_policy(BudgetPolicy::new().with_max_cost(10.0))
        .with_pre_turn_policy(MaxTurnsPolicy::new(5))
        .with_pre_dispatch_policy(ToolDenyListPolicy::new(["bash"]));

    let mut agent = Agent::new(options);
    let result = agent.prompt_text("Hello!").await?;
    println!("{}", result.assistant_text());
    Ok(())
}
```

See `cargo run -p swink-agent-policies --example with_policies` for a runnable end-to-end demo.

## Recommended Production Guardrails

The library default is anything-goes — no policy slots are populated. Embedders running agents autonomously in production should wire the recommended guardrail set. The `recommended` feature bundles it in one call:

```rust,ignore
use swink_agent_policies::RecommendedPolicies;

let options = RecommendedPolicies::builder()
    .with_max_cost(10.0)                      // default: $10 USD
    .with_max_turns(50)                       // default: 50 turns
    .with_sandbox_root("/srv/agent-workspace") // default: process cwd
    .with_deny_tools(["bash"])                // default: ["bash"]
    .apply(options);
```

This wires `BudgetPolicy` + `MaxTurnsPolicy` (pre-turn) and `SandboxPolicy` + `ToolDenyListPolicy` (pre-dispatch), appending to any policies already present.

To catch accidental guardrail removal in downstream code, run the integration-contract helper in your test suite against your real `AgentOptions` construction:

```rust,ignore
use swink_agent_policies::assert_production_guardrails;

#[test]
fn production_options_keep_their_guardrails() {
    let options = build_my_production_options();
    // Panics unless all four policies are present with non-trivial limits
    // and "bash" is actually denied.
    assert_production_guardrails(&options, "bash");
}
```

`verify_production_guardrails` is the non-panicking variant, returning every violation as a `Vec<String>`. Both check presence by canonical policy name and probe each policy behaviorally (extreme cost/turn counts, an escape path, the denied tool), so a `BudgetPolicy` with no limits or a sandbox rooted at `/` fails the contract.

## Crash Safety

Nothing is persisted by default — conversation history lives in memory, and a crashed process loses the session (spec 031, FR-019: no policy is enabled unless the embedder opts in). Opting in takes two lines: a durable store plus a post-turn checkpoint policy (`checkpoint` feature here, `swink-agent-memory` for the store):

```rust,ignore
use swink_agent_memory::FileCheckpointStore;
use swink_agent_policies::RollingCheckpointPolicy;

let dir = FileCheckpointStore::default_dir().expect("config dir"); // <config_dir>/swink-agent/checkpoints
let store = Arc::new(FileCheckpointStore::new(dir)?);
let options = options
    .with_post_turn_policy(RollingCheckpointPolicy::new(store).with_session_id(&session_id));
```

Pick the policy by what you need back after a crash:

- **`RollingCheckpointPolicy`** — recommended for long sessions. Overwrites **one** checkpoint per turn via the store's atomic write path, so disk cost is O(context) regardless of session length; on a crash you lose at most one turn. No per-turn history.
- **`CheckpointPolicy`** — one checkpoint **per turn** (time-travel restore), each containing the full history to date, so an N-turn session stores O(N²) bytes. Scope IDs with `.with_session_id(...)` — without it, IDs are `turn-{n}` and a second `prompt()` run reuses (and partially overwrites) the first run's IDs, which can make "restore the latest turn" resurrect stale history. Pair with `FileCheckpointStore::with_max_checkpoints(n)` to cap disk usage.

To restore, set the same store on `AgentOptions::with_checkpoint_store` and use the agent's `load_and_restore_checkpoint` path.

## Architecture

Every policy implements one of the four `Policy*` traits exposed by the core crate. When a slot fires, the loop iterates policies in registration order; the first `PolicyDecision::Stop` short-circuits the turn. Policies never mutate agent state directly — they return structured decisions the core loop acts on, which keeps their behavior testable in isolation and composable without ordering surprises beyond the declared slot.

No `unsafe` code (`#![forbid(unsafe_code)]`). Application policies (`pii`, `prompt-guard`, `content-filter`) use compiled regexes with bounded memory; patterns are validated at policy construction time, not per-call.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
