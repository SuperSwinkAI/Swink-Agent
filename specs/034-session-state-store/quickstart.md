# Quickstart: Session Key-Value State Store

**Feature**: 034-session-state-store
**Date**: 2026-03-31

## Basic Usage

### Pre-seed state and read after conversation

```rust
use swink_agent::{Agent, AgentOptions, SessionState};
use std::sync::RwLock;

// Pre-seed state via builder
let options = AgentOptions::new(/* ... */)
    .with_state_entry("user_id", "abc123")
    .with_state_entry("preferences", serde_json::json!({"theme": "dark"}));

let mut agent = Agent::new(options);

// Run conversation
agent.prompt_async("Find me restaurants nearby").await?;

// Read state after conversation
let state_lock = agent.session_state();
let state = state_lock.read().unwrap();
if let Some(count) = state.get::<i64>("results_found") {
    println!("Agent found {} results", count);
}
```

### Tool that reads and writes state

```rust
use swink_agent::{AgentTool, AgentToolResult, SessionState};
use std::sync::{Arc, RwLock};

struct SearchTool;

impl AgentTool for SearchTool {
    fn name(&self) -> &str { "search" }
    // ... other trait methods ...

    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
        state: Arc<RwLock<SessionState>>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>> {
        Box::pin(async move {
            // Read previously stored URLs to avoid duplicates
            let known_urls: Vec<String> = {
                let s = state.read().unwrap();
                s.get("visited_urls").unwrap_or_default()
            };

            // ... perform search, filter out known URLs ...
            let new_urls = vec!["https://example.com".to_string()];

            // Write updated state
            {
                let mut s = state.write().unwrap();
                let mut all_urls = known_urls;
                all_urls.extend(new_urls);
                s.set("visited_urls", &all_urls);
                s.set("last_search_query", params["query"].as_str().unwrap_or(""));
            }

            AgentToolResult::text("Found 3 new results")
        })
    }
}
```

### State-aware policy

```rust
use swink_agent::policy::{PreTurnPolicy, PolicyContext, PolicyVerdict};

struct RequireVerifiedPolicy;

impl PreTurnPolicy for RequireVerifiedPolicy {
    fn name(&self) -> &str { "require-verified" }

    fn evaluate(&self, ctx: &PolicyContext) -> PolicyVerdict {
        match ctx.state.get::<bool>("verified") {
            Some(true) => PolicyVerdict::Continue,
            _ => PolicyVerdict::Stop("User not verified".into()),
        }
    }
}
```

### Session persistence with state

```rust
use swink_agent_memory::{JsonlSessionStore, SessionStore, SessionMeta};

let store = JsonlSessionStore::new(sessions_dir)?;

// After conversation — save state alongside messages
let state_lock = agent.session_state();
let state = state_lock.read().unwrap();
store.save_state("session-001", &state.snapshot())?;

// Later — restore session with state
if let Some(snapshot) = store.load_state("session-001")? {
    let restored_state = SessionState::restore_from_snapshot(snapshot);
    let options = AgentOptions::new(/* ... */)
        .with_initial_state(restored_state);
    let agent = Agent::new(options);
    // Agent now has full state from previous session
}
```

### Subscribing to state changes

```rust
agent.subscribe(|event| {
    if let AgentEvent::StateChanged { delta } = event {
        for (key, value) in &delta.changes {
            match value {
                Some(v) => println!("State set: {} = {}", key, v),
                None => println!("State removed: {}", key),
            }
        }
    }
});
```

## Key Points

- **Thread-safe**: Use `state.read().unwrap()` for reads, `state.write().unwrap()` for writes
- **Typed access**: `state.get::<T>(key)` deserializes; returns `None` on missing key or type mismatch
- **Raw access**: `state.get_raw(key)` returns `Option<&Value>` without deserialization
- **Delta tracking**: Automatic — every `set`/`remove` records a delta entry
- **Flush**: Delta flushed automatically at turn end; `StateChanged` event emitted if non-empty
- **Pre-seeding**: Builder methods add initial state without recording delta
- **Persistence**: Full snapshot strategy — call `save_state` with `state.snapshot()`
