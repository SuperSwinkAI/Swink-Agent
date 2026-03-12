# Tool System

**Source files:** `src/tool.rs`, `src/tools/`
**Related:** [PRD §4](../../planning/PRD.md#4-tool-system)

The tool system defines how tools are declared, validated, executed, and how their results are returned to the LLM. It also covers the structured output mechanism, which is implemented as a synthetic tool injected by the harness.

---

## L2 — Components

```mermaid
flowchart TB
    subgraph CallerLayer["👤 Caller"]
        ToolImpl["Tool Implementations<br/>(AgentTool trait)"]
    end

    subgraph ToolSystem["🔧 Tool System"]
        Registry["Tool Registry<br/>(held in AgentContext)"]
        Validator["Argument Validator<br/>(JSON Schema)"]
        Executor["Concurrent Executor<br/>(tokio::spawn per call)"]
        StructuredTool["StructuredOutputTool<br/>(synthetic — injected by harness)"]
    end

    subgraph Loop["🔄 Agent Loop"]
        ToolDispatch["Tool Dispatch<br/>(routes tool_call_id → tool)"]
    end

    subgraph LLM["🌐 LLM Provider"]
        Model["Model"]
    end

    ToolImpl --> Registry
    StructuredTool --> Registry
    Registry --> ToolDispatch
    ToolDispatch --> Validator
    Validator -->|"valid"| Executor
    Validator -->|"invalid — error result, no execute"| ToolDispatch
    Executor --> ToolImpl
    ToolImpl -->|"AgentToolResult"| Executor
    Executor -->|"ToolResultMessage"| Loop
    Registry -->|"tool schemas"| Model

    classDef callerStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef toolStyle fill:#ff9800,stroke:#e65100,stroke-width:2px,color:#000
    classDef loopStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000
    classDef externalStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000

    class ToolImpl callerStyle
    class Registry,Validator,Executor,StructuredTool toolStyle
    class ToolDispatch loopStyle
    class Model externalStyle
```

---

## L3 — Built-in Tools

The harness ships three built-in tools in `src/tools/`. They are ordinary `AgentTool` implementations and can be registered alongside caller-provided tools.

### Shared constant: `MAX_OUTPUT_BYTES`

Defined in `src/tools/mod.rs` as `100 * 1024` (100 KB). Used by `BashTool` and `ReadFileTool` to truncate output before returning it to the LLM, preventing oversized context windows.

### BashTool (`src/tools/bash.rs`)

Executes arbitrary shell commands via `sh -c`.

| Field | Value |
|---|---|
| **name** | `bash` |
| **Parameters** | `command` (string, required), `timeout_ms` (integer, optional — default 30 000 ms) |
| **Output** | Exit code + stdout + stderr. Combined stdout/stderr truncated at `MAX_OUTPUT_BYTES`, split proportionally with stdout favoured. |
| **Cancellation** | Checks `cancellation_token.is_cancelled()` before spawning. During execution, `tokio::select!` races the child process against both the cancellation token and the timeout. On cancellation or timeout the child process is killed. |
| **Security note** | Runs arbitrary commands via `sh -c`. Not suitable for agents exposed to untrusted users. |

### ReadFileTool (`src/tools/read_file.rs`)

Reads a file and returns its contents as text.

| Field | Value |
|---|---|
| **name** | `read_file` |
| **Parameters** | `path` (string, required — absolute path) |
| **Output** | File contents as text, truncated at `MAX_OUTPUT_BYTES` with a `[truncated]` marker appended. |
| **Cancellation** | Checks `cancellation_token.is_cancelled()` before the read. Returns error immediately if cancelled. |

### WriteFileTool (`src/tools/write_file.rs`)

Writes content to a file, creating parent directories as needed.

| Field | Value |
|---|---|
| **name** | `write_file` |
| **Parameters** | `path` (string, required — absolute path), `content` (string, required) |
| **Output** | Success message including byte count written. |
| **Cancellation** | Checks `cancellation_token.is_cancelled()` before the write. Returns error immediately if cancelled. |

### Tool cancellation pattern

All built-in tools follow the same cancellation contract:

1. **Pre-check** — Before starting any I/O, the tool calls `cancellation_token.is_cancelled()` and returns an error result immediately if true.
2. **During work** — `BashTool` uses `tokio::select!` to race the child process against `cancellation_token.cancelled()`, killing the process on cancellation. The file tools perform a single async operation, so the pre-check is sufficient.

### `ToolExecutionUpdate` event

The `on_update` callback parameter in `execute()` is designed for tools that produce streaming partial results (e.g., long-running commands emitting incremental output). `ToolExecutionUpdate` events are defined in the event model but **currently reserved for future use** — the agent loop always passes `None` for the callback. Built-in tools accept but ignore the parameter.

---

## L3 — AgentTool Trait Contract

```mermaid
flowchart LR
    subgraph Trait["AgentTool (trait)"]
        Name["name() → &str<br/>unique routing key"]
        Label["label() → &str<br/>human-readable display name"]
        Desc["description() → &str<br/>natural language for LLM prompt"]
        Schema["parameters_schema() → &Value<br/>JSON Schema for validation"]
        Execute["execute(<br/>  tool_call_id: &str,<br/>  params: Value,<br/>  cancellation_token: CancellationToken,<br/>  on_update: Option&lt;Fn(AgentToolResult)&gt;<br/>) → Future&lt;AgentToolResult&gt;"]
    end

    subgraph Result["AgentToolResult"]
        Content["content: Vec&lt;ContentBlock&gt;<br/>(Text | Image — returned to LLM)"]
        Details["details: Value<br/>(structured data for logging,<br/>not sent to LLM)"]
    end

    Execute --> Result

    classDef traitStyle fill:#ff9800,stroke:#e65100,stroke-width:2px,color:#000
    classDef resultStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000

    class Name,Label,Desc,Schema,Execute traitStyle
    class Content,Details resultStyle
```

---

## L3 — Argument Validation Pipeline

Before `execute` is called, arguments from the LLM are validated against the tool's JSON Schema. Failures produce an error result without touching the implementation.

```mermaid
flowchart LR
    subgraph Input["📥 Input"]
        ToolCall["ToolCall content block<br/>from AssistantMessage<br/>(id, name, arguments: Value)"]
    end

    subgraph Lookup["🔍 Registry Lookup"]
        FindTool{"tool found<br/>by name?"}
        UnknownErr["error AgentToolResult<br/>unknown tool"]
    end

    subgraph Validation["✅ Validation"]
        ValidateArgs{"arguments match<br/>JSON Schema?"}
        ValidationErr["error AgentToolResult<br/>argument validation failed<br/>(field-level detail)"]
    end

    subgraph Execution["⚡ Execution"]
        Spawn["tokio::spawn<br/>execute()"]
        UpdateCallback["on_update callback<br/>(optional streaming updates)"]
        ToolResult["AgentToolResult<br/>(content + details)"]
    end

    subgraph Output["📤 Output"]
        ToolResultMsg["ToolResultMessage<br/>(appended to context)"]
    end

    ToolCall --> FindTool
    FindTool -->|"not found"| UnknownErr
    FindTool -->|"found"| ValidateArgs
    ValidateArgs -->|"invalid"| ValidationErr
    ValidateArgs -->|"valid"| Spawn
    Spawn --> UpdateCallback
    Spawn --> ToolResult
    UnknownErr --> ToolResultMsg
    ValidationErr --> ToolResultMsg
    ToolResult --> ToolResultMsg

    classDef inputStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef validStyle fill:#ff9800,stroke:#e65100,stroke-width:2px,color:#000
    classDef errorStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000
    classDef execStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef outputStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000

    class ToolCall inputStyle
    class FindTool,ValidateArgs validStyle
    class UnknownErr,ValidationErr errorStyle
    class Spawn,UpdateCallback,ToolResult execStyle
    class ToolResultMsg outputStyle
```

---

## L3 — Concurrent Tool Execution

When an assistant message contains multiple tool calls, the harness spawns them concurrently. Each tool receives its own `CancellationToken` (a child of the loop's token). When steering arrives after a tool completes, all remaining in-flight tools are cancelled via their `CancellationToken`, and for each cancelled tool an error `ToolResultMessage` is injected with content: `"tool call cancelled: user requested steering interrupt"`.

```mermaid
flowchart TB
    subgraph AssistantTurn["AssistantMessage"]
        TC1["ToolCall A"]
        TC2["ToolCall B"]
        TC3["ToolCall C"]
    end

    subgraph Executor["Concurrent Executor"]
        Spawn1["tokio::spawn → Tool A<br/>(child CancellationToken)"]
        Spawn2["tokio::spawn → Tool B<br/>(child CancellationToken)"]
        Spawn3["tokio::spawn → Tool C<br/>(child CancellationToken)"]
    end

    subgraph Results["Results (as they complete)"]
        R1["Result A → poll steering"]
        R2["Result B → poll steering"]
        R3["Result C → poll steering"]
    end

    subgraph Steering["Steering Check"]
        S1{"steering?"}
        S2{"steering?"}
        S3{"steering?"}
    end

    TC1 --> Spawn1
    TC2 --> Spawn2
    TC3 --> Spawn3
    Spawn1 --> R1
    Spawn2 --> R2
    Spawn3 --> R3
    R1 --> S1
    R2 --> S2
    R3 --> S3
    S1 -->|"yes"| CancelBC["cancel B, C via CancellationToken,<br/>inject error ToolResultMessages"]
    S1 -->|"no"| Continue1["continue"]
    S2 -->|"yes"| CancelC["cancel C via CancellationToken,<br/>inject error ToolResultMessage"]
    S2 -->|"no"| Continue2["continue"]
    S3 -->|"any"| Done["all complete"]

    classDef msgStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef execStyle fill:#ff9800,stroke:#e65100,stroke-width:2px,color:#000
    classDef resultStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000
    classDef steerStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef skipStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000

    class TC1,TC2,TC3 msgStyle
    class Spawn1,Spawn2,Spawn3 execStyle
    class R1,R2,R3 resultStyle
    class S1,S2,S3 steerStyle
    class CancelBC,CancelC,Continue1,Continue2,Done skipStyle
```

---

## L4 — Structured Output Flow

> **Note:** Structured output is managed by the `Agent` struct, not the loop. The `Agent` injects the synthetic tool, runs the loop normally, validates the result, and retries via `continue_loop()` if invalid. The loop itself has no structured output awareness.

Structured output is implemented as a synthetic tool injected alongside the caller's tools. The model is instructed to call it as its final action.

```mermaid
sequenceDiagram
    participant App as Application
    participant Agent as Agent Struct
    participant RunLoop as run_loop
    participant LLM as LLM Provider
    participant Validator as Schema Validator

    App->>Agent: structured_output(prompt, schema)
    Agent->>Agent: create StructuredOutputTool from schema
    Agent->>Agent: inject into tool registry for this call
    Agent->>RunLoop: launch loop with modified system prompt<br/>("you must call structured_output as your final action")

    RunLoop->>LLM: stream turn
    LLM-->>RunLoop: AssistantMessage with ToolCall(structured_output, data)

    RunLoop->>Validator: validate data against schema
    alt valid
        Validator-->>RunLoop: Ok(Value)
        RunLoop-->>Agent: AgentResult with structured_output: Value
        Agent-->>App: Ok(typed result)
    else invalid, retries remaining
        Validator-->>RunLoop: Err(field errors)
        RunLoop->>RunLoop: inject validation error as user message
        RunLoop->>LLM: retry turn
    else max retries exceeded
        RunLoop-->>Agent: Err(StructuredOutputFailed)
        Agent-->>App: Err
    end
```
