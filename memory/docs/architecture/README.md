# Memory Architecture — Multi-Layer Vision

## Overview

The swink-agent memory system is designed as a layered architecture where each layer handles a different scope of persistence and retrieval. Lower layers are simpler and more concrete; higher layers add intelligence and are the focus of ongoing research.

## Layer Model

```
┌─────────────────────────────────────────────────────────┐
│  Layer 5: Explicit Memory Tools                         │
│  Agent-driven remember(k,v) / recall(query)             │
├─────────────────────────────────────────────────────────┤
│  Layer 4: Tool-Aware Compaction                         │
│  Weight tool results by type (ephemeral vs reference)   │
├─────────────────────────────────────────────────────────┤
│  Layer 3: Semantic Memory / RAG                         │
│  Vector store over past sessions, embedding-based       │
├─────────────────────────────────────────────────────────┤
│  Layer 2: Summarization Compaction          [stub]      │
│  LLM-generated summaries of dropped context             │
├─────────────────────────────────────────────────────────┤
│  Layer 1: Session Persistence               [done]      │
│  JSONL save/load/list/delete via SessionStore trait      │
├─────────────────────────────────────────────────────────┤
│  Layer 0: Sliding Window (core crate)       [done]      │
│  Anchor + tail compaction with token budgets            │
└─────────────────────────────────────────────────────────┘
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

### Layer 3 — Semantic Memory / RAG (future)

Store embeddings of past session messages in a vector database. On each turn, retrieve relevant context from past sessions and inject it into the conversation. This enables cross-session memory without loading entire histories.

**Open questions:**
- Embedding model selection (local vs API)
- Vector store (in-process like `hnsw` vs external like Qdrant)
- Retrieval granularity (message-level vs turn-level vs summary-level)
- Injection mechanism (via `ConvertToLlmFn` or as `CustomMessage`)

### Layer 4 — Tool-Aware Compaction (future)

Not all tool results are equal. A `bash` output showing `ls` is ephemeral; a `read_file` result may be reference material needed later. Tool-aware compaction assigns weights to tool results based on their type and content, keeping reference material longer.

**Open questions:**
- Classification heuristic (tool name? content length? recency of reference?)
- Integration with Layer 2 (summarize ephemeral results, preserve reference ones?)
- Per-tool retention policies

### Layer 5 — Explicit Memory Tools (future)

Give the agent `remember(key, value)` and `recall(query)` tools. The agent decides what to store based on conversation context. This is the most autonomous layer — the agent manages its own long-term memory.

**Open questions:**
- Storage backend (same as Layer 3? separate key-value store?)
- Recall ranking (exact match, fuzzy, embedding-based?)
- Memory capacity limits and eviction policies
- Trust model (should the agent's memory decisions be auditable/editable by users?)

## Integration Points with Core

| Core Hook | Used By | Purpose |
|-----------|---------|---------|
| `TransformContextFn` | Layers 0, 2, 4 | Synchronous context mutation per turn |
| `ConvertToLlmFn` | Layer 3 | Inject retrieved context as LLM messages |
| `CustomMessage` | Layers 3, 5 | Store metadata that survives compaction |
| `AgentTool` | Layer 5 | `remember` and `recall` tool implementations |
| `AgentEvent` | Layers 2, 3 | Trigger async processing (summarization, indexing) |
