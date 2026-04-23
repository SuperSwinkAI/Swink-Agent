# swink-agent-memory

[![Crates.io](https://img.shields.io/crates/v/swink-agent-memory.svg)](https://crates.io/crates/swink-agent-memory)
[![Docs.rs](https://docs.rs/swink-agent-memory/badge.svg)](https://docs.rs/swink-agent-memory)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

Session persistence and context management for [`swink-agent`](https://crates.io/crates/swink-agent) — durable conversation storage, recoverable checkpoints, and summarization-based compaction.

## Features

- **`JsonlSessionStore`** — append-only JSONL session storage with atomic writes and a resumable sequence counter
- **`FileCheckpointStore`** — durable agent-state checkpoints between turns (for crash recovery and mid-run inspection)
- **`SummarizingCompactor`** — LLM-driven context compaction that preserves recent turns and summarizes older ones
- **`SessionMigrator`** — forward-compatible session schema migration with versioned meta headers
- **`InterruptState`** / **`PendingToolCall`** — capture in-flight tool calls across restarts
- **`BlockingSessionStore`** — sync wrapper for non-async callers
- `format_session_id()` produces sortable, timestamped IDs like `20260320_143000_6f00bfe3f7c54b2f86d780df58ccf0a1`

## Quick Start

```toml
[dependencies]
swink-agent = "0.8"
swink-agent-memory = "0.8"
tokio = { version = "1", features = ["full"] }
```

```rust,ignore
use swink_agent_memory::{
    JsonlSessionStore, SessionMeta, SessionStore, format_session_id, now_utc,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = JsonlSessionStore::default_dir()?;
    let store = JsonlSessionStore::new(dir)?;

    let id = format_session_id();
    let meta = SessionMeta {
        id: id.clone(),
        title: "My session".into(),
        created_at: now_utc(),
        updated_at: now_utc(),
        version: 1,
        sequence: 0,
    };
    store.save(&id, &meta, &messages)?;
    Ok(())
}
```

## Architecture

A session is two files side by side: a `.meta.json` header and a `.jsonl` append-only log of agent messages. `JsonlSessionStore` writes each record with a monotonically-increasing sequence number so a crash mid-write leaves a recoverable state. `FileCheckpointStore` uses the same atomic-write pattern for whole-agent snapshots. `SummarizingCompactor` plugs into the core crate's `ContextTransformer` trait — it asks the current model to summarize old turns when the window gets tight, keeping the last N turns verbatim.

No `unsafe` code (`#![forbid(unsafe_code)]`). All writes are atomic (temp-file + rename); partial writes cannot corrupt a session.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
