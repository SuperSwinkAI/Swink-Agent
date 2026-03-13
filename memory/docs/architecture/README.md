# Memory Architecture

## Overview

The swink-agent memory system is designed as a layered architecture where each layer handles a different scope of persistence and retrieval. Higher layers (RAG, tool-aware compaction, explicit memory tools) are planned for a separate research repository.

## Layer Model

```
┌─────────────────────────────────────────────────────────┐
│  Layer 2: Summarization Compaction          [stub]      │
│  LLM-generated summaries of dropped context             │
├─────────────────────────────────────────────────────────┤
│  Layer 1: Session Persistence               [done]      │
│  JSONL save/load/list/delete via SessionStore trait      │
├─────────────────────────────────────────────────────────┤
│  Layer 0: Sliding Window (core crate)       [done]      │
│  Anchor + tail compaction with token budgets            │
└─────────────────────────────────────────────────────────┘

Higher layers (RAG, tool-aware compaction, explicit memory tools)
live in a separate research repository.
```

## Layer Details

### Layer 0 — Sliding Window (core)

Lives in `swink-agent/src/context.rs`. Purely mechanical: keeps anchor messages and as many recent messages as fit within a token budget. No intelligence, no persistence.

**Status:** Complete and tested.

### Layer 1 — Session Persistence

Lives in `swink-agent-memory/src/{store,jsonl,meta}.rs`. Saves and loads full conversation histories as JSONL files. The `SessionStore` trait allows alternative backends.

**Status:** Complete. Extracted from TUI.

### Layer 2 — Summarization Compaction

Lives in `swink-agent-memory/src/compaction.rs`. Wraps the sliding window with a pre-computed summary that replaces dropped messages. The summary is generated asynchronously (outside the agent loop) and injected synchronously during compaction.

**Key design constraint:** `TransformContextFn` is synchronous. Summaries must be pre-computed async and stored for later injection.

**Status:** Stub implemented. Summary injection works; LLM-powered summary generation is future work.

## Integration Points with Core

| Core Hook | Used By | Purpose |
|-----------|---------|---------|
| `TransformContextFn` | Layers 0, 2 | Synchronous context mutation per turn |
| `AgentEvent` | Layer 2 | Trigger async processing (summarization) |
