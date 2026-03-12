# Memory Research — Experiment Plans

## Experiment 1: Summarization Quality

**Goal:** Determine the best strategy for summarizing dropped context.

**Variables:**
- Prompt template: structured (bullet points) vs narrative vs extractive (key facts only)
- Model: same model as conversation vs dedicated cheap model (e.g. haiku)
- Summary length: fixed budget (500 tokens) vs proportional (10% of dropped content)
- Freshness: regenerate every compaction vs incremental (append to existing summary)

**Metrics:**
- Task completion rate on multi-turn benchmarks with/without summaries
- Summary faithfulness (does it preserve facts the agent needs later?)
- Latency overhead per turn
- Token cost of summary generation

**Design:**
1. Build a benchmark: 10 multi-turn conversations that exceed context window
2. Baseline: sliding window only (Layer 0)
3. Compare: summary injection with different prompt templates
4. Measure: can the agent correctly answer questions about earlier context?

## Experiment 2: RAG Over Past Sessions

**Goal:** Enable cross-session memory via retrieval-augmented generation.

**Variables:**
- Embedding model: `text-embedding-3-small` vs local (e.g. `nomic-embed-text`)
- Chunk granularity: per-message, per-turn (user+assistant), per-session-summary
- Vector store: in-process (`hnsw_rs`, `usearch`) vs external (Qdrant, ChromaDB)
- Retrieval count: top-3, top-5, top-10
- Injection format: system prompt addendum vs user message prefix

**Metrics:**
- Retrieval precision/recall on known-answer queries
- End-to-end task accuracy with/without RAG
- Latency per retrieval call
- Storage growth rate

**Design:**
1. Build a corpus: 50+ sessions from TUI usage (anonymized)
2. Index at different granularities
3. Test: "What did we decide about X last week?" style queries
4. Compare: no memory, full session load, RAG retrieval

## Experiment 3: Tool-Aware Compaction

**Goal:** Retain high-value tool results longer than ephemeral ones.

**Hypotheses:**
- `read_file` results are referenced more often than `bash` output
- Tool results referenced by later assistant messages should be preserved
- Large tool results (>1000 tokens) should be summarized rather than dropped entirely

**Variables:**
- Classification: by tool name, by content size, by reference count
- Retention multiplier: 2x, 5x, 10x normal weight for reference tools
- Summarization: summarize large results vs truncate vs drop

**Design:**
1. Instrument the TUI to log which tool results the agent references in later turns
2. Build a reference frequency model per tool type
3. Implement weighted token estimation in a custom `TransformContextFn`
4. Compare task completion with/without tool-aware weighting

## Experiment 4: Explicit Memory Tools

**Goal:** Let the agent manage its own persistent memory.

**Design considerations:**
- `remember(key: str, value: str)` — store a fact with a semantic key
- `recall(query: str) -> Vec<(key, value)>` — retrieve relevant facts
- `forget(key: str)` — remove a stored fact

**Variables:**
- Storage: flat JSON file vs SQLite vs vector-indexed
- Recall: exact key match, fuzzy match, embedding similarity
- Capacity: unlimited vs fixed (evict LRU), with/without agent awareness of limits
- Prompting: explicit instruction to use memory tools vs emergent behavior

**Metrics:**
- Does the agent use `remember` without being told to?
- Does the agent recall relevant facts when they would help?
- Memory pollution rate (useless or incorrect entries)
- User satisfaction with memory-augmented agents

**Open risk:** The agent may over-use `remember`, storing noise. Need eviction or quality filtering.

## Priority Order

1. **Summarization** (Experiment 1) — lowest risk, highest immediate value, builds on existing stub
2. **Tool-aware compaction** (Experiment 3) — low risk, improves context quality
3. **RAG** (Experiment 2) — medium risk, requires embedding infrastructure
4. **Explicit memory tools** (Experiment 4) — highest risk, most research-heavy, but highest long-term payoff
