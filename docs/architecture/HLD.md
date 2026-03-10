# Agent Harness — High Level Design

**Related Documents:**
- Product Requirements: [PRD.md](../planning/PRD.md)

---

## System Overview

The Agent Harness is a pure-Rust library crate that provides the core scaffolding for building LLM-powered agentic applications. It is consumed as a dependency by calling applications; it has no runtime process of its own. The harness manages the agent loop, message context, tool dispatch, streaming, and lifecycle events. All LLM provider access is delegated to a caller-supplied `StreamFn` implementation, keeping the harness fully provider-agnostic.

---

## C4 Level 1 — System Context

This diagram shows the agent harness as a single system and the external actors and systems it interacts with.

```mermaid
flowchart TB
    subgraph CallerLayer["👤 Caller"]
        App["Calling Application<br/>(Rust binary or library)"]
    end

    subgraph HarnessSystem["⚙️ Agent Harness (Rust Library)"]
        Harness["agent-harness<br/>Agent loop, tool dispatch,<br/>streaming, events, retry"]
    end

    subgraph ExternalSystems["🌐 External Systems"]
        LLMProvider["LLM Provider API<br/>(Anthropic, OpenAI, Gemini, …)"]
        ProxyServer["LLM Proxy Server<br/>(optional — auth + routing)"]
    end

    App -->|"Constructs Agent,<br/>supplies StreamFn + Tools"| Harness
    Harness -->|"AgentEvent stream,<br/>AgentResult"| App
    Harness -->|"Streaming inference<br/>via StreamFn (direct)"| LLMProvider
    Harness -->|"Streaming inference<br/>via ProxyStreamFn (SSE)"| ProxyServer
    ProxyServer -->|"Proxied request"| LLMProvider

    classDef callerStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef harnessStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef externalStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000

    class App callerStyle
    class Harness harnessStyle
    class LLMProvider,ProxyServer externalStyle
```

**Key relationships**

| Relationship | Direction | Description |
|---|---|---|
| App → Harness | Inbound | Caller constructs an `Agent`, registers tools, supplies a `StreamFn`, and invokes prompts |
| Harness → App | Outbound | Harness emits `AgentEvent` values and returns `AgentResult` on completion |
| Harness → LLM Provider | Outbound | Direct streaming inference via caller-supplied `StreamFn` |
| Harness → Proxy Server | Outbound | Optional: built-in `ProxyStreamFn` forwards requests to a proxy over SSE |
| Proxy Server → LLM Provider | Outbound | Proxy handles auth and routes to the actual provider |

---

## Internal Component Architecture

This diagram shows the major internal modules and how they relate within the harness.

```mermaid
flowchart TB
    subgraph CallerLayer["👤 Caller"]
        App["Calling Application"]
        Tools["Tool Implementations<br/>(AgentTool trait)"]
        StreamImpl["StreamFn Implementation<br/>(direct or proxy)"]
    end

    subgraph AgentLayer["⚙️ Agent Struct"]
        Agent["Agent<br/>State management<br/>Steering + follow-up queues<br/>Sync / async / streaming API"]
    end

    subgraph LoopLayer["🔄 Agent Loop"]
        Loop["run_loop<br/>Turn orchestration<br/>Tool concurrency<br/>Steering + follow-up handling"]
    end

    subgraph ToolLayer["🔧 Tool System"]
        Validator["Argument Validator<br/>(JSON Schema)"]
        Executor["Concurrent Executor<br/>(tokio::spawn per call)"]
    end

    subgraph StreamLayer["📡 Streaming Interface"]
        StreamFn["StreamFn Trait<br/>(provider-agnostic)"]
        ProxyFn["ProxyStreamFn<br/>(SSE + delta reconstruction)"]
    end

    subgraph InfraLayer["🏗️ Infrastructure"]
        Events["Event System<br/>(AgentEvent enum)"]
        Retry["Retry Strategy<br/>(exp. back-off + jitter)"]
        Cancel["Cancellation<br/>(CancellationToken)"]
        Errors["Error Types<br/>(ContextWindowOverflow,<br/>MaxTokensReached)"]
    end

    subgraph ExternalLayer["🌐 External"]
        LLMProvider["LLM Provider API"]
        ProxyServer["LLM Proxy Server"]
    end

    App -->|"prompt / invoke"| Agent
    App --> Tools
    App --> StreamImpl
    Agent -->|"agent_loop /<br/>agent_loop_continue"| Loop
    Loop -->|"validate + execute"| Validator
    Validator --> Executor
    Executor --> Tools
    Loop -->|"call StreamFn"| StreamFn
    StreamFn --> StreamImpl
    StreamImpl -->|direct| LLMProvider
    ProxyFn -->|SSE| ProxyServer
    ProxyServer --> LLMProvider
    Loop -->|"emit"| Events
    Events -->|"subscribe"| App
    Loop --> Retry
    Loop --> Cancel
    Loop --> Errors

    classDef callerStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef agentStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef loopStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef toolStyle fill:#ff9800,stroke:#e65100,stroke-width:2px,color:#000
    classDef streamStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef infraStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000
    classDef externalStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000

    class App,Tools,StreamImpl callerStyle
    class Agent agentStyle
    class Loop loopStyle
    class Validator,Executor toolStyle
    class StreamFn,ProxyFn streamStyle
    class Events,Retry,Cancel,Errors infraStyle
    class LLMProvider,ProxyServer externalStyle
```

---

## Single Turn Data Flow

This diagram traces the path of a single prompt through the harness from invocation to completion.

```mermaid
flowchart LR
    subgraph CallerLayer["👤 Caller"]
        App["Application"]
    end

    subgraph AgentLayer["⚙️ Agent"]
        Agent["Agent Struct"]
        Queue["Steering /<br/>Follow-up Queues"]
    end

    subgraph LoopLayer["🔄 Loop"]
        TurnStart["Emit TurnStart"]
        StreamCall["Call StreamFn"]
        MsgEvents["Emit MessageStart<br/>MessageUpdate ×N<br/>MessageEnd"]
        ToolCheck["Extract Tool Calls"]
        ToolExec["Execute Tools<br/>(concurrent)"]
        SteerPoll["Poll Steering<br/>Messages"]
        TurnEnd["Emit TurnEnd"]
        FollowPoll["Poll Follow-up<br/>Messages"]
        AgentEnd["Emit AgentEnd"]
    end

    subgraph InfraLayer["🏗️ Infrastructure"]
        Retry["Retry Strategy"]
        Errors["Error Recovery"]
    end

    subgraph ExternalLayer["🌐 External"]
        LLM["LLM Provider"]
    end

    App -->|"prompt()"| Agent
    Agent --> Queue
    Agent --> TurnStart
    TurnStart --> StreamCall
    StreamCall -->|"retryable failure"| Retry
    Retry --> StreamCall
    StreamCall <-->|"SSE delta stream"| LLM
    StreamCall --> MsgEvents
    MsgEvents --> ToolCheck
    ToolCheck -->|"stop_reason: length"| Errors
    Errors --> ToolExec
    ToolCheck -->|"tool calls present"| ToolExec
    ToolExec --> SteerPoll
    SteerPoll -->|"steering arrived"| TurnStart
    SteerPoll -->|"no steering"| TurnEnd
    TurnEnd --> FollowPoll
    FollowPoll -->|"follow-up arrived"| TurnStart
    FollowPoll -->|"none"| AgentEnd
    AgentEnd -->|"AgentResult"| App

    classDef callerStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef agentStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef loopStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000
    classDef infraStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef externalStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000

    class App callerStyle
    class Agent,Queue agentStyle
    class TurnStart,StreamCall,MsgEvents,ToolCheck,ToolExec,SteerPoll,TurnEnd,FollowPoll,AgentEnd loopStyle
    class Retry,Errors infraStyle
    class LLM externalStyle
```

---

## Crate Module Dependencies

This diagram shows how the source modules depend on each other, reflecting the build order.

```mermaid
flowchart TB
    subgraph FoundationLayer["🏗️ Foundation"]
        types["types.rs<br/>AgentMessage, ContentBlock,<br/>ModelSpec, AgentResult, Usage"]
        error["error.rs<br/>HarnessError,<br/>ContextWindowOverflow,<br/>MaxTokensReached"]
    end

    subgraph CoreLayer["⚙️ Core Abstractions"]
        tool["tool.rs<br/>AgentTool trait,<br/>AgentToolResult,<br/>argument validation"]
        stream["stream.rs<br/>StreamFn trait,<br/>StreamOptions,<br/>AssistantMessageEvent,<br/>AssistantMessageDelta"]
        retry["retry.rs<br/>RetryStrategy trait,<br/>default implementation"]
    end

    subgraph ImplLayer["🔧 Implementations"]
        proxy["proxy.rs<br/>ProxyStreamFn,<br/>SSE delta reconstruction"]
    end

    subgraph ExecutionLayer["🔄 Execution"]
        loop_["loop_.rs<br/>agent_loop,<br/>agent_loop_continue,<br/>run_loop,<br/>AgentLoopConfig"]
    end

    subgraph APILayer["📦 Public API"]
        agent["agent.rs<br/>Agent struct,<br/>AgentOptions"]
        lib["lib.rs<br/>public re-exports"]
    end

    types --> tool
    types --> stream
    types --> retry
    error --> tool
    error --> loop_
    tool --> loop_
    stream --> loop_
    stream --> proxy
    retry --> loop_
    loop_ --> agent
    proxy --> agent
    agent --> lib
    loop_ --> lib
    types --> lib

    classDef foundationStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000
    classDef coreStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef implStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000
    classDef execStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef apiStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000

    class types,error foundationStyle
    class tool,stream,retry coreStyle
    class proxy implStyle
    class loop_ execStyle
    class agent,lib apiStyle
```

---

## Design Decisions

**Library, not a service.** The harness is a crate, not a daemon. There are no HTTP ports, no config files, no CLI. Callers link it as a dependency and own the runtime.

**StreamFn is the only provider boundary.** All LLM communication flows through a single trait. Direct providers, proxies, mock implementations for testing, and future transports all satisfy the same interface. The harness never holds an API key or SDK client.

**Events are outward-only.** The event system is a push channel from the harness to the caller. Hooks that mutate execution (cancel a tool, retry a call) are expressed as callbacks in `AgentLoopConfig`, not as event responses. This avoids re-entrant state.

**Errors stay in the message log.** LLM and tool errors produce assistant messages rather than unwinding the call stack. The caller always gets a complete, inspectable message history regardless of outcome.

**Concurrency is scoped to tool execution.** Tool calls within a single turn run concurrently via `tokio::spawn`. Everything else — turns, steering polls, follow-up polls — is sequential. This makes the loop easy to reason about without sacrificing the main performance win of parallel tool execution.
