# Quickstart: Memory Crate

**Branch**: `021-memory-crate` | **Date**: 2026-03-20

## Add the dependency

```toml
# In your crate's Cargo.toml
[dependencies]
swink-agent-memory = { path = "../memory" }
```

## Save and load a session (sync)

```rust
use swink_agent_memory::{JsonlSessionStore, SessionMeta, SessionStore};
use swink_agent_memory::time::{now_utc, format_session_id};

// Create a store backed by a local directory
let store = JsonlSessionStore::new("./sessions")?;

// Build session metadata
let id = format_session_id(); // e.g., "20260320_143000"
let meta = SessionMeta {
    id: id.clone(),
    title: "Debug session".into(),
    created_at: now_utc(),
    updated_at: now_utc(),
};

// Save a conversation
let messages = vec![/* your LlmMessage values */];
store.save(&id, &meta, &messages)?;

// Load it back
let (loaded_meta, loaded_messages) = store.load(&id)?;
assert_eq!(loaded_messages.len(), messages.len());
```

## Save and load a session (async)

```rust
use swink_agent_memory::{JsonlSessionStore, AsyncSessionStore, SessionMeta};

let store = JsonlSessionStore::new("./sessions")?;
let id = "20260320_143000";

// Async save
store.save(id, &meta, &messages).await?;

// Async load
let (meta, messages) = store.load(id).await?;
```

## Append messages incrementally

```rust
// After a new turn, append just the new messages
let new_messages = vec![/* latest user + assistant messages */];
store.append(&id, &new_messages)?;
```

## List and delete sessions

```rust
// List all sessions
let sessions: Vec<SessionMeta> = store.list()?;
for s in &sessions {
    println!("{}: {} (updated {})", s.id, s.title, s.updated_at);
}

// Delete a session
store.delete("20260320_143000")?;
```

## Set up summarizing compaction

```rust
use swink_agent_memory::SummarizingCompactor;

// Create a compactor that retains ~4000 tokens of recent messages
let compactor = SummarizingCompactor::new(4000);

// Wire it into the agent's transform_context
let agent = Agent::new(/* ... */)
    .with_transform_context(compactor.compaction_fn());

// After each turn, pre-compute the summary asynchronously
// (using your own LLM call) and store it:
let summary = my_summarize_fn(&old_messages).await?;
compactor.set_summary(summary);

// On the next turn, the compaction_fn closure will inject
// the summary as an AssistantMessage at the start of the
// retained window, replacing older messages.
```

## Build and test

```bash
cargo build -p swink-agent-memory
cargo test -p swink-agent-memory
```
