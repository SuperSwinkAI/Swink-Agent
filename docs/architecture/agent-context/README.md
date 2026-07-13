# Agent Context

**Source files:** `src/context.rs` (sliding window compaction), `src/types/`, `src/context_transformer.rs`, `src/async_context_transformer.rs`
**Related:** [PRD §5](../../planning/PRD.md#5-agent-context)

The agent context is the immutable snapshot passed into each loop turn. It contains the system prompt, the current message history, and the list of available tools. The loop never mutates a context in place — each turn produces a new snapshot.

---

## L2 — Structure

```mermaid
flowchart TB
    subgraph AgentContext["AgentContext"]
        SP["system_prompt: String"]
        Msgs["messages: Vec&lt;AgentMessage&gt;"]
        Tools["tools: Vec&lt;Arc&lt;dyn AgentTool&gt;&gt;"]
    end

    subgraph LoopState["LoopState (internal)"]
        CtxMsgs["context_messages: Vec&lt;AgentMessage&gt;"]
        Overflow["overflow_signal: bool"]
    end

    subgraph Consumers["Consumers"]
        AsyncTransformCtx["async_transform_context<br/>AsyncContextTransformer trait (async)<br/>runs BEFORE the sync transform"]
        TransformCtx["transform_context<br/>ContextTransformer trait (sync)<br/>transform(&amp;mut Vec&lt;AgentMessage&gt;, bool)<br/>→ Option&lt;CompactionReport&gt;"]
        ConvertLlm["convert_to_llm<br/>Fn(&amp;AgentMessage) → Option&lt;LlmMessage&gt;"]
        StreamFn["StreamFn<br/>receives &amp;AgentContext"]
    end

    LoopState -->|"(&amp;mut context_messages, overflow_signal)"| AsyncTransformCtx
    AsyncTransformCtx --> TransformCtx
    TransformCtx --> ConvertLlm
    ConvertLlm -->|"LlmMessages built"| AgentContext
    AgentContext --> StreamFn

    classDef contextStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef stateStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef consumerStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef fieldStyle fill:#fafafa,stroke:#bdbdbd,stroke-width:1px,color:#000

    class SP,Msgs,Tools fieldStyle
    class CtxMsgs,Overflow stateStyle
    class AsyncTransformCtx,TransformCtx,ConvertLlm,StreamFn consumerStyle
```

---

### L3 — Two-Context Design

The implementation creates two distinct `AgentContext` instances per turn:

1. **Initial context** — Built in `run_turn` with an empty `messages` vec. Carries `system_prompt` and `tools` so they are available to `stream_with_retry`.
2. **Call context** — Built inside `stream_with_retry` (and rebuilt on each retry attempt). Populates `messages` with the LLM-filtered messages (`Vec<AgentMessage>` wrapping the `LlmMessage` values produced by `convert_to_llm`). This is the context actually passed to `StreamFn`.

Message transformation and LLM conversion happen *before* either context is constructed — they operate directly on `LoopState.context_messages`.

---

### L3 — Per-Turn Snapshot Lifecycle

```mermaid
sequenceDiagram
    participant State as LoopState
    participant AsyncTransform as async_transform_context
    participant Transform as transform_context
    participant Convert as convert_to_llm
    participant InitCtx as AgentContext (initial)
    participant CallCtx as AgentContext (call)
    participant Stream as StreamFn

    Note over State: — Turn N begins —

    State->>AsyncTransform: &mut context_messages, overflow_signal
    Note over AsyncTransform: async I/O allowed — summary fetching,<br/>RAG retrieval, database lookups<br/>(AsyncContextTransformer, if configured)

    AsyncTransform->>Transform: &mut context_messages, overflow_signal
    Note over Transform: may prune / reorder / inject tokens<br/>(ContextTransformer — synchronous)

    Note over State: overflow_signal reset to false

    Transform->>Convert: mutated context_messages
    Note over Convert: AgentMessage → Option<LlmMessage><br/>(drops Custom variants, etc.)

    Convert->>InitCtx: initial context built<br/>(system_prompt, tools, messages: empty)

    Note over CallCtx: call context built inside stream_with_retry<br/>(system_prompt, tools, messages: LlmMessages)
    CallCtx->>Stream: &AgentContext with LLM messages
    Note over Stream: streams assistant response

    Stream-->>State: assistant message + tool results<br/>appended to context_messages

    Note over State: — Turn N+1 begins —
```

---

### L3 — Overflow Signal

The overflow signal is managed internally in `LoopState` — it is **not** a field on `AgentContext`. It is passed as a plain `bool` parameter to both transformer traits.

- When a `ContextWindowOverflow` error is detected, the loop sets `LoopState.overflow_signal = true` and continues to the next inner-loop iteration.
- At the start of the next turn, the transformers are called with `overflow = true`, so they can apply more aggressive pruning (e.g., dropping older tool results, summarising earlier turns).
- Immediately after, `overflow_signal` is reset to `false` — the signal is consumed in a single turn.
- The signal never flows through `AgentContext`; it exists only in `LoopState` and is passed directly to the transformers.

How the overflow interacts with the surrounding turn/retry machinery is shown in the [agent-loop README](../agent-loop/README.md).

---

## Related: Memory Crate

The `swink-agent-memory` crate builds on the `ContextTransformer` / `AsyncContextTransformer` hooks to provide higher-level compaction strategies. `SummarizingCompactor` wraps `sliding_window` and injects pre-computed summaries of dropped messages after the anchor. See [`memory/docs/architecture/`](../../../memory/docs/architecture/README.md) for the multi-layer memory vision.
