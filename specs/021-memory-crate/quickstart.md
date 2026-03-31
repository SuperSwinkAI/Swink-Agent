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

## Save rich session entries

```rust
use swink_agent_memory::{SessionEntry, JsonlSessionStore, SessionStore};

// Rich entries alongside messages
let entries = vec![
    SessionEntry::Message(user_message),
    SessionEntry::Message(assistant_message),
    SessionEntry::ModelChange {
        from: old_model,
        to: new_model,
        timestamp: 1711900000,
    },
    SessionEntry::Label {
        text: "Important decision here".into(),
        message_index: 1,
        timestamp: 1711900100,
    },
];

// Rich entries are NOT sent to the LLM — only Message entries
```

## Persist and resume from interrupts

```rust
use swink_agent_memory::{InterruptState, PendingToolCall, SessionStore};

// Save interrupt state when agent is paused at approval gate
let interrupt = InterruptState {
    interrupted_at: 1711900000,
    pending_tool_calls: vec![PendingToolCall {
        tool_call_id: "tc_001".into(),
        tool_name: "bash".into(),
        arguments: serde_json::json!({"command": "ls -la"}),
    }],
    context_snapshot: agent_messages.clone(),
    system_prompt: "Be helpful.".into(),
    model: current_model.clone(),
};
store.save_interrupt(&session_id, &interrupt)?;

// On restart, check for pending interrupt
if let Some(state) = store.load_interrupt(&session_id)? {
    // Resume from interrupt state
    println!("Resuming with {} pending tool calls", state.pending_tool_calls.len());
}

// Clear after resuming
store.clear_interrupt(&session_id)?;
```

## Load a filtered subset of a session

```rust
use swink_agent_memory::LoadOptions;

// Only load the last 20 entries
let options = LoadOptions {
    last_n_entries: Some(20),
    ..Default::default()
};
let (meta, entries) = store.load_with_options(&session_id, &options)?;

// Only load message entries (skip model changes, labels, etc.)
let options = LoadOptions {
    entry_types: Some(vec!["message".into()]),
    ..Default::default()
};
```

## Build and test

```bash
cargo build -p swink-agent-memory
cargo test -p swink-agent-memory
```
