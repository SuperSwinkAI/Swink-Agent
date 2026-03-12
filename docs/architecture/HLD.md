# Swink Agent — High Level Design

**Related Documents:**
- Product Requirements: [PRD.md](../planning/PRD.md)

---

## System Overview

The Swink Agent is a Rust workspace composed of three crates that provide the core scaffolding for building LLM-powered agentic applications. The **core library** (`swink-agent`) manages the agent loop, message context, tool dispatch, streaming, and lifecycle events. The **adapters crate** (`swink-agent-adapters`) provides ready-made `StreamFn` implementations for specific LLM providers. The **TUI crate** (`swink-agent-tui`) is a binary that provides an interactive terminal interface. All LLM provider access is delegated to a `StreamFn` implementation, keeping the core harness fully provider-agnostic.

---

## C4 Level 1 — System Context

This diagram shows the swink agent as a single system and the external actors and systems it interacts with.

```mermaid
flowchart TB
    subgraph CallerLayer["👤 Caller"]
        App["Calling Application<br/>(Rust binary or library)"]
        TUI["Terminal UI<br/>(ratatui + crossterm)"]
    end

    subgraph HarnessSystem["⚙️ Swink Agent (Rust Workspace)"]
        Harness["swink-agent (core)<br/>Agent loop, tool dispatch,<br/>streaming, events, retry"]
        Adapters["swink-agent-adapters<br/>LLM provider adapters<br/>(Ollama, Anthropic, OpenAI)"]
    end

    subgraph ExternalSystems["🌐 External Systems"]
        LLMProvider["LLM Provider API<br/>(Anthropic, OpenAI, Gemini, …)"]
        ProxyServer["LLM Proxy Server<br/>(optional — auth + routing)"]
    end

    App -->|"Constructs Agent,<br/>supplies StreamFn + Tools"| Harness
    Harness -->|"AgentEvent stream,<br/>AgentResult"| App
    TUI -->|"Agent API +<br/>event subscription"| Harness
    TUI -->|"Uses adapter"| Adapters
    Harness -->|"AgentEvent stream"| TUI
    Adapters -->|"Streaming inference<br/>via OllamaStreamFn (NDJSON)"| LLMProvider
    Harness -->|"Streaming inference<br/>via ProxyStreamFn (SSE)"| ProxyServer
    ProxyServer -->|"Proxied request"| LLMProvider

    classDef callerStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef harnessStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef externalStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000

    class App,TUI callerStyle
    class Harness,Adapters harnessStyle
    class LLMProvider,ProxyServer externalStyle
```

**Key relationships**

| Relationship | Direction | Description |
|---|---|---|
| App → Harness | Inbound | Caller constructs an `Agent`, registers tools, supplies a `StreamFn`, and invokes prompts |
| Harness → App | Outbound | Harness emits `AgentEvent` values and returns `AgentResult` on completion |
| Adapters → LLM Provider | Outbound | `OllamaStreamFn` streams inference via Ollama's `/api/chat` endpoint (NDJSON) |
| Harness → Proxy Server | Outbound | Optional: built-in `ProxyStreamFn` forwards requests to a proxy over SSE |
| Proxy Server → LLM Provider | Outbound | Proxy handles auth and routes to the actual provider |
| TUI → Adapters | Internal | TUI selects provider by priority: Proxy (LLM_BASE_URL), OpenAI (OPENAI_API_KEY), Anthropic (ANTHROPIC_API_KEY), Ollama (default) |

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

    subgraph AdapterLayer["🔌 Adapters Crate"]
        OllamaFn["OllamaStreamFn<br/>(NDJSON streaming)"]
        AnthropicFn["AnthropicStreamFn<br/>(SSE + thinking blocks)"]
        OpenAiFn["OpenAiStreamFn<br/>(SSE, multi-provider)"]
    end

    subgraph InfraLayer["🏗️ Infrastructure"]
        Events["Event System<br/>(AgentEvent enum)"]
        Retry["Retry Strategy<br/>(exp. back-off + jitter)"]
        Cancel["Cancellation<br/>(CancellationToken)"]
        Errors["Error Types<br/>(ContextWindowOverflow,<br/>ModelThrottled, StreamError)"]
    end

    subgraph TUILayer["🖥️ Terminal UI"]
        TUIApp["TUI App<br/>Event loop, layout,<br/>focus management"]
        ConvView["Conversation View<br/>Message rendering,<br/>markdown, syntax highlighting"]
        InputEditor["Input Editor<br/>Multi-line text input"]
        ToolPanel["Tool Panel<br/>Active executions, results"]
        StatusBar["Status Bar<br/>Model, usage, state"]
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
    OllamaFn -->|"implements"| StreamFn
    AnthropicFn -->|"implements"| StreamFn
    OpenAiFn -->|"implements"| StreamFn
    StreamImpl -->|direct| LLMProvider
    OllamaFn -->|"NDJSON"| LLMProvider
    AnthropicFn -->|"SSE"| LLMProvider
    OpenAiFn -->|"SSE"| LLMProvider
    ProxyFn -->|SSE| ProxyServer
    ProxyServer --> LLMProvider
    Loop -->|"emit"| Events
    Events -->|"subscribe"| App
    Loop --> Retry
    Loop --> Cancel
    Loop --> Errors
    TUIApp -->|"prompt / abort"| Agent
    Events -->|"subscribe"| TUIApp
    TUIApp --> ConvView
    TUIApp --> InputEditor
    TUIApp --> ToolPanel
    TUIApp --> StatusBar

    classDef callerStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef agentStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef loopStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef toolStyle fill:#ff9800,stroke:#e65100,stroke-width:2px,color:#000
    classDef streamStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef adapterStyle fill:#c8e6c9,stroke:#388e3c,stroke-width:2px,color:#000
    classDef infraStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000
    classDef externalStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000
    classDef tuiStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff

    class App,Tools,StreamImpl callerStyle
    class Agent agentStyle
    class Loop loopStyle
    class Validator,Executor toolStyle
    class StreamFn,ProxyFn streamStyle
    class OllamaFn,AnthropicFn,OpenAiFn adapterStyle
    class Events,Retry,Cancel,Errors infraStyle
    class TUIApp,ConvView,InputEditor,ToolPanel,StatusBar tuiStyle
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
    StreamCall <-->|"SSE or NDJSON<br/>delta stream"| LLM
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

## Workspace Crate Dependencies

This diagram shows how the three workspace crates and their internal modules depend on each other.

```mermaid
flowchart TB
    subgraph CoreCrate["📦 swink-agent (core)"]
        subgraph FoundationLayer["🏗️ Foundation"]
            types["types.rs<br/>AgentMessage, ContentBlock,<br/>ModelSpec, AgentResult, Usage"]
            error["error.rs<br/>AgentError,<br/>ContextWindowOverflow,<br/>ModelThrottled, StreamError"]
        end

        subgraph CoreLayer["⚙️ Core Abstractions"]
            tool["tool.rs<br/>AgentTool trait,<br/>AgentToolResult,<br/>argument validation"]
            stream["stream.rs<br/>StreamFn trait,<br/>StreamOptions,<br/>AssistantMessageEvent,<br/>AssistantMessageDelta"]
            retry["retry.rs<br/>RetryStrategy trait,<br/>default implementation"]
        end

        subgraph ImplLayer["🔧 Implementations"]
            proxy["proxy.rs<br/>ProxyStreamFn,<br/>SSE delta reconstruction"]
            context["context.rs<br/>sliding_window,<br/>overflow-aware pruning"]
            builtintools["tools/<br/>BashTool, ReadFileTool,<br/>WriteFileTool"]
        end

        subgraph ExecutionLayer["🔄 Execution"]
            loop_["loop_.rs<br/>agent_loop,<br/>agent_loop_continue,<br/>run_loop,<br/>AgentLoopConfig"]
        end

        subgraph APILayer["📦 Public API"]
            agent["agent.rs<br/>Agent struct,<br/>AgentOptions"]
            lib["lib.rs<br/>public re-exports"]
        end
    end

    subgraph AdaptersCrate["🔌 swink-agent-adapters"]
        adapters_lib["lib.rs<br/>re-exports"]
        ollama["ollama.rs<br/>OllamaStreamFn,<br/>NDJSON streaming"]
        anthropic["anthropic.rs<br/>AnthropicStreamFn,<br/>SSE + thinking blocks"]
        openai["openai.rs<br/>OpenAiStreamFn,<br/>SSE, multi-provider"]
        convert["convert.rs<br/>MessageConverter trait"]
    end

    subgraph TUICrate["🖥️ swink-agent-tui"]
        tui_main["main.rs<br/>env var config,<br/>provider selection"]
        tui_app["app.rs<br/>event loop, layout"]
        tui_creds["credentials.rs<br/>keychain integration"]
        tui_session["session.rs<br/>session persistence"]
        tui_wizard["wizard.rs<br/>first-run setup"]
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

    stream -->|"StreamFn trait"| ollama
    stream -->|"StreamFn trait"| anthropic
    stream -->|"StreamFn trait"| openai
    ollama --> adapters_lib
    anthropic --> adapters_lib
    openai --> adapters_lib
    convert --> ollama
    convert --> openai

    lib -->|"swink-agent dep"| tui_main
    adapters_lib -->|"adapters dep"| tui_main
    tui_main --> tui_app

    classDef foundationStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000
    classDef coreStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef implStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000
    classDef execStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef apiStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef adapterStyle fill:#c8e6c9,stroke:#388e3c,stroke-width:2px,color:#000
    classDef tuiStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff

    class types,error foundationStyle
    class tool,stream,retry coreStyle
    class proxy,context,builtintools implStyle
    class loop_ execStyle
    class agent,lib apiStyle
    class adapters_lib,ollama,anthropic,openai,convert adapterStyle
    class tui_main,tui_app,tui_creds,tui_session,tui_wizard tuiStyle
```

---

## Design Decisions

**Library, not a service.** The harness is a crate, not a daemon. There are no HTTP ports, no config files, no CLI. Callers link it as a dependency and own the runtime.

**StreamFn is the only provider boundary.** All LLM communication flows through a single trait. Direct providers, proxies, mock implementations for testing, and future transports all satisfy the same interface. The harness never holds an API key or SDK client. Four built-in implementations ship with the project: `ProxyStreamFn` (SSE, in core), `OllamaStreamFn` (NDJSON), `AnthropicStreamFn` (SSE with thinking blocks), and `OpenAiStreamFn` (SSE, multi-provider compatible) — the latter three in the adapters crate.

**Adapters are a separate crate.** Provider-specific `StreamFn` implementations live in `swink-agent-adapters`, keeping the core harness free of any provider SDK or protocol detail. Adding a new provider means adding a module to the adapters crate — no changes to the core.

**Events are outward-only.** The event system is a push channel from the harness to the caller. Hooks that mutate execution (cancel a tool, retry a call) are expressed as callbacks in `AgentLoopConfig`, not as event responses. This avoids re-entrant state.

**Errors stay in the message log.** LLM and tool errors produce assistant messages rather than unwinding the call stack. The caller always gets a complete, inspectable message history regardless of outcome.

**Concurrency is scoped to tool execution.** Tool calls within a single turn run concurrently via `tokio::spawn`. Everything else — turns, steering polls, follow-up polls — is sequential. This makes the loop easy to reason about without sacrificing the main performance win of parallel tool execution.

**TUI is a separate crate.** The terminal interface is a binary crate that depends on both the core library and the adapters crate, not a feature-gated module. This keeps the core harness free of terminal dependencies and allows the TUI to evolve independently. The TUI consumes the same public API that any other application would use.

## TUI Architecture

The TUI is a separate binary crate (`swink-agent-tui`) that depends on both `swink-agent` (core) and `swink-agent-adapters`. It provides an interactive terminal interface for conversing with an LLM agent. The TUI supports four providers (Proxy, OpenAI, Anthropic, Ollama) selected by environment variable priority. It includes a first-run setup wizard for API key configuration, session persistence, and credential management via the system keychain.

### Provider Configuration

The TUI selects its LLM provider via environment variables in priority order: Proxy > OpenAI > Anthropic > Ollama. API keys can also be stored in the system keychain via the `#key` command or the first-run setup wizard.

| Variable | Default | Description |
|---|---|---|
| `LLM_BASE_URL` | _(unset)_ | SSE proxy endpoint — highest priority if set |
| `LLM_API_KEY` | _(empty)_ | Bearer token for the proxy |
| `LLM_MODEL` | `claude-sonnet-4-20250514` | Model identifier for the proxy |
| `OPENAI_API_KEY` | _(unset)_ | OpenAI API key (or keychain) |
| `OPENAI_BASE_URL` | `https://api.openai.com` | OpenAI-compatible endpoint |
| `OPENAI_MODEL` | `gpt-4o` | OpenAI model name |
| `ANTHROPIC_API_KEY` | _(unset)_ | Anthropic API key (or keychain) |
| `ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | Anthropic endpoint |
| `ANTHROPIC_MODEL` | `claude-sonnet-4-20250514` | Anthropic model name |
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama server URL (default fallback) |
| `OLLAMA_MODEL` | `llama3.2` | Ollama model name |
| `LLM_SYSTEM_PROMPT` | `You are a helpful assistant.` | System prompt (shared across all providers) |

### Component Model

The TUI uses a component-based architecture where each UI element is a stateful widget rendered via `ratatui`. The component tree is:

```
App
├── Conversation View (scrollable message history)
│   ├── User Message Block
│   ├── Assistant Message Block (with streaming)
│   │   ├── Text Content (markdown rendered)
│   │   ├── Thinking Block (dimmed)
│   │   └── Tool Call Block
│   └── Tool Result Block
├── Input Editor (multi-line text composition)
├── Tool Panel (active tool executions)
└── Status Bar (model, usage, state)
```

### Event Loop

The TUI runs a dual event loop:

1. **Terminal events** — `crossterm` delivers keyboard, mouse, and resize events. These are dispatched to the focused component for input handling.
2. **Agent events** — The TUI subscribes to `AgentEvent` from the harness via `Agent::subscribe`. Events arrive on a channel and trigger UI state updates.

Both event sources are multiplexed via `tokio::select!` in the main render loop.

### Data Flow

```mermaid
flowchart LR
    subgraph Terminal["🖥️ Terminal"]
        Stdin["stdin<br/>(keyboard, mouse)"]
        Stdout["stdout<br/>(rendered frames)"]
    end

    subgraph TUI["TUI App"]
        EventLoop["Event Loop<br/>(tokio::select!)"]
        State["App State<br/>(messages, focus, scroll)"]
        Renderer["ratatui Renderer"]
    end

    subgraph Harness["Swink Agent"]
        Agent["Agent"]
        Events["AgentEvent Stream"]
    end

    Stdin -->|"crossterm events"| EventLoop
    EventLoop -->|"update"| State
    State -->|"render"| Renderer
    Renderer -->|"draw"| Stdout
    EventLoop -->|"prompt / abort"| Agent
    Events -->|"subscribe"| EventLoop

    classDef termStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000
    classDef tuiStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef harnessStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000

    class Stdin,Stdout termStyle
    class EventLoop,State,Renderer tuiStyle
    class Agent,Events harnessStyle
```
