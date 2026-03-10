# Agent Context

**Source file:** `src/types.rs`
**Related:** [PRD §5](../../planning/PRD.md#5-agent-context)

The agent context is the immutable snapshot passed into each loop turn. It contains the system prompt, the current message history, and the list of available tools. The loop never mutates a context in place — each turn produces a new snapshot.

---

## L2 — Structure

```mermaid
flowchart TB
    subgraph AgentContext["📸 AgentContext"]
        SP["system_prompt: String"]
        Msgs["messages: Vec&lt;AgentMessage&gt;"]
        Tools["tools: Vec&lt;Arc&lt;dyn AgentTool&gt;&gt;"]
        Overflow["overflow: bool"]
    end

    subgraph AgentState["⚙️ AgentState"]
        StateFields["system_prompt · messages · tools<br/>(mutable, long-lived)"]
    end

    subgraph Consumers["📤 Consumers"]
        TransformCtx["transform_context<br/>async Fn(&amp;mut Vec&lt;AgentMessage&gt;, bool) → Vec&lt;AgentMessage&gt;"]
        ConvertLlm["convert_to_llm<br/>Fn(&amp;AgentMessage) → Option&lt;LlmMessage&gt;"]
        StreamFn["StreamFn<br/>receives &amp;AgentContext"]
    end

    AgentState -->|"clone relevant fields<br/>(start of each turn)"| AgentContext
    AgentContext --> TransformCtx
    TransformCtx --> ConvertLlm
    ConvertLlm --> StreamFn

    classDef contextStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef stateStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef consumerStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef fieldStyle fill:#fafafa,stroke:#bdbdbd,stroke-width:1px,color:#000

    class SP,Msgs,Tools,Overflow fieldStyle
    class StateFields stateStyle
    class TransformCtx,ConvertLlm,StreamFn consumerStyle
```

---

### L3 — Immutability Strategy

- `AgentContext` is constructed at the start of each turn by cloning relevant fields from `AgentState`.
- The context is passed by shared reference (`&AgentContext`) to `StreamFn` and hooks.
- Messages produced during the turn (assistant message, tool results) are appended to `AgentState`, not to the context snapshot.
- This ensures the `StreamFn` and hooks always see a consistent view, even during concurrent tool execution.

---

### L3 — Per-Turn Snapshot Lifecycle

```mermaid
sequenceDiagram
    participant State as AgentState
    participant Ctx as AgentContext
    participant Transform as transform_context
    participant Convert as convert_to_llm
    participant Stream as StreamFn

    Note over State: — Turn N begins —
    State->>Ctx: create snapshot<br/>(clone system_prompt, messages, tools, overflow)

    Ctx->>Transform: &mut messages from snapshot
    Note over Transform: may prune / reorder / inject tokens

    Transform->>Convert: processed messages
    Note over Convert: AgentMessage → Option<LlmMessage><br/>(drops Custom variants, etc.)

    Convert->>Stream: &AgentContext (with mapped messages)
    Note over Stream: streams assistant response

    Stream-->>State: assistant message + tool results<br/>appended to AgentState

    Note over State: — Turn N+1 begins —
    State->>Ctx: new snapshot from updated AgentState
```

---

### L3 — Overflow Signal

When a `ContextWindowOverflow` error occurs, the harness records this state and uses the next snapshot to communicate the condition to downstream hooks.

- When a `ContextWindowOverflow` error occurs, the harness records this state on `AgentState`.
- On retry via `continue_loop()`, the new `AgentContext` snapshot carries an `overflow` flag set to `true`.
- `transform_context` receives this flag and can apply more aggressive pruning (e.g., dropping older tool results, summarising earlier turns).
- After successful recovery (the next turn completes without overflow), the flag is cleared on `AgentState`, and subsequent snapshots revert to `overflow: false`.

```mermaid
sequenceDiagram
    participant Loop as run_loop
    participant State as AgentState
    participant Ctx as AgentContext
    participant Transform as transform_context

    Note over Loop: Turn fails with ContextWindowOverflow
    Loop->>State: set overflow = true

    Note over Loop: continue_loop() called
    State->>Ctx: create snapshot (overflow: true)
    Ctx->>Transform: &mut messages, overflow = true
    Note over Transform: aggressive pruning applied

    Transform->>Loop: pruned messages
    Note over Loop: turn succeeds

    Loop->>State: set overflow = false
    Note over State: next snapshot will have overflow: false
```
