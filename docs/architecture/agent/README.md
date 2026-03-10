# Agent Struct

**Source file:** `src/agent.rs`
**Related:** [PRD §13](../../planning/PRD.md#13-agent-struct)

The `Agent` struct is the primary public interface of the harness. It is a stateful wrapper over the agent loop that owns conversation history, manages the steering and follow-up queues, exposes three invocation modes, and fans lifecycle events out to subscribers.

---

## L2 — Responsibilities

```mermaid
flowchart TB
    subgraph CallerLayer["👤 Caller"]
        App["Application"]
    end

    subgraph AgentLayer["⚙️ Agent Struct"]
        State["AgentState<br/>system_prompt · model · tools<br/>messages · running flag<br/>stream_message · error"]
        Queues["Message Queues<br/>steering_queue<br/>follow_up_queue"]
        API["Invocation API<br/>prompt_stream()<br/>prompt_async()<br/>prompt_sync()<br/>structured_output()<br/>continue_loop()"]
        Events["Event Subscriptions<br/>listener registry<br/>subscribe / unsubscribe"]
        Control["Control<br/>abort() · reset()<br/>wait_for_idle()"]
    end

    subgraph LoopLayer["🔄 Agent Loop"]
        Loop["run_loop"]
    end

    App -->|"constructs + configures"| State
    App -->|"prompt / continue"| API
    App -->|"steer / follow_up"| Queues
    App -->|"subscribe"| Events
    App -->|"abort / reset"| Control
    API -->|"builds AgentLoopConfig<br/>launches loop"| Loop
    Queues -->|"drained each turn"| Loop
    Loop -->|"emits AgentEvent"| Events
    Events -->|"fan-out to callbacks"| App
    Loop -->|"appends messages"| State
    Control -->|"signals CancellationToken"| Loop

    classDef callerStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef agentStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef loopStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000

    class App callerStyle
    class State,Queues,API,Events,Control agentStyle
    class Loop loopStyle
```

---

## L3 — AgentState Fields

```mermaid
flowchart LR
    subgraph AgentState["AgentState"]
        SP["system_prompt: String"]
        Model["model: ModelSpec"]
        Tools["tools: Vec&lt;Arc&lt;dyn AgentTool&gt;&gt;"]
        Messages["messages: Vec&lt;AgentMessage&gt;"]
        Running["is_running: bool"]
        StreamMsg["stream_message: Option&lt;AgentMessage&gt;"]
        PendingTools["pending_tool_calls: HashSet&lt;String&gt;"]
        Error["error: Option&lt;String&gt;"]
    end

    subgraph Owned["Owned by Agent (private)"]
        SteerQ["steering_queue: Vec&lt;AgentMessage&gt;"]
        FollowQ["follow_up_queue: Vec&lt;AgentMessage&gt;"]
        Listeners["listeners: HashMap&lt;SubscriptionId, Box&lt;dyn Fn(&AgentEvent)&gt;&gt;"]
        Cancel["abort_controller: Option&lt;CancellationToken&gt;"]
        Running2["running_promise: Option&lt;JoinHandle&gt;"]
        StreamMode["steering_mode: SteeringMode"]
        FollowMode["follow_up_mode: FollowUpMode"]
    end

    classDef stateStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef ownedStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000

    class SP,Model,Tools,Messages,Running,StreamMsg,PendingTools,Error stateStyle
    class SteerQ,FollowQ,Listeners,Cancel,Running2,StreamMode,FollowMode ownedStyle
```

---

## L3 — Invocation Modes

All three prompt variants and structured output share the same underlying `run_loop`. The differences are in how the result is surfaced to the caller.

```mermaid
flowchart TB
    subgraph Input["📥 Input variants"]
        TextInput["Text(String)"]
        TextImages["Text + Images"]
        Messages["Vec&lt;AgentMessage&gt;"]
    end

    subgraph Modes["📤 Invocation modes"]
        Streaming["prompt_stream()<br/>→ Stream&lt;AgentEvent&gt;<br/>caller consumes events"]
        Async["prompt_async()<br/>→ Future&lt;AgentResult&gt;<br/>awaits completion"]
        Sync["prompt_sync()<br/>→ AgentResult<br/>blocks calling thread"]
        Structured["structured_output(schema)<br/>→ Future&lt;Value&gt;<br/>validates against JSON Schema"]
        Continue["continue_loop()<br/>→ (streaming | async | sync)<br/>resumes from existing context"]
    end

    subgraph Core["🔄 Core"]
        Loop["run_loop"]
    end

    TextInput --> Streaming
    TextInput --> Async
    TextInput --> Sync
    TextImages --> Streaming
    TextImages --> Async
    TextImages --> Sync
    Messages --> Streaming
    Messages --> Async
    Messages --> Sync
    Streaming --> Loop
    Async --> Loop
    Sync --> Loop
    Structured --> Loop
    Continue --> Loop

    classDef inputStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef modeStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef coreStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff

    class TextInput,TextImages,Messages inputStyle
    class Streaming,Async,Sync,Structured,Continue modeStyle
    class Loop coreStyle
```

> **Note — Structured output** is owned by the `Agent` struct. The `Agent` injects a synthetic tool, runs the loop, validates the result against the JSON Schema, and retries via `continue_loop()` if invalid — up to a configurable maximum. The loop itself has no structured output awareness.

---

## L4 — Concurrency State Machine

The `Agent` permits only one active invocation at a time. This state machine governs transitions.

```mermaid
stateDiagram-v2
    [*] --> Idle : constructed

    Idle --> Running : prompt() / continue_loop()
    Running --> Idle : loop completes (AgentEnd)
    Running --> Idle : abort() → StopReason::Aborted
    Running --> Idle : unrecoverable error

    Idle --> Idle : steer() [queued, no effect until next run]
    Idle --> Idle : follow_up() [queued]
    Running --> Running : steer() [enqueued, drained after next tool batch]
    Running --> Running : follow_up() [enqueued, drained when loop would stop]

    Idle --> Idle : reset() [clears state + queues]
    Running --> Running : ERROR — prompt() rejected, returns Err
```

---

## L4 — Steering and Follow-up Queue Draining

```mermaid
sequenceDiagram
    participant App as Application
    participant Agent as Agent Struct
    participant Loop as run_loop

    App->>Agent: prompt("do something")
    Agent->>Loop: launch with get_steering_messages callback

    Note over Loop: executing tool calls...

    App->>Agent: steer(message)
    Note over Agent: pushed to steering_queue

    Loop->>Agent: poll get_steering_messages()
    Agent-->>Loop: [steering message]
    Note over Loop: skip remaining tools,<br/>inject steering msg,<br/>start new turn

    Note over Loop: agent reaches natural stop...

    Loop->>Agent: poll get_follow_up_messages()
    Agent-->>Loop: [] (empty)
    Loop-->>Agent: AgentEnd
    Agent-->>App: AgentResult
```

> **Note:** On error or abort, follow-up queues are NOT polled — the loop exits immediately.
