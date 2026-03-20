# Error Handling

**Source files:** `src/error.rs`, `src/retry.rs`, `src/loop_.rs`
**Related:** [PRD §10](../../planning/PRD.md#10-error-handling), [PRD §11](../../planning/PRD.md#11-retry-strategy)

The harness distinguishes three categories of failure: recoverable model errors (surfaced in the message log), typed operational errors (context overflow), transient provider failures (handled by the retry strategy), and fatal errors. No category results in a panic.

---

## L2 — Error Categories

```mermaid
flowchart TB
    subgraph Category1["📝 In-Message Errors"]
        MsgErr["LLM / tool errors<br/>appended as AssistantMessage<br/>stop_reason: error<br/>error_message: String"]
        ToolValErr["Tool validation errors<br/>returned as error AgentToolResult<br/>(no execute() call)"]
    end

    subgraph Category2["🔴 Typed Operational Errors"]
        CWO["ContextWindowOverflow<br/>input exceeds model context window<br/>history preserved for caller"]
    end

    subgraph Category3["🔁 Transient Failures (Retry)"]
        Throttle["AgentError::ModelThrottled<br/>rate limit / 429 from provider"]
        Network["AgentError::NetworkError<br/>transient IO / connection failure"]
        Retry["→ RetryStrategy<br/>exponential back-off + jitter"]
    end

    subgraph Category4["💥 Fatal Errors"]
        Fatal["Unrecoverable<br/>(bad config, logic bugs)<br/>→ AgentError returned to caller<br/>loop exits cleanly"]
    end

    classDef msgStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef typeStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef retryStyle fill:#ff9800,stroke:#e65100,stroke-width:2px,color:#000
    classDef fatalStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000

    class MsgErr,ToolValErr msgStyle
    class CWO typeStyle
    class Throttle,Network,Retry retryStyle
    class Fatal fatalStyle
```

---

## L3 — Error Classification from Stream Events

When the stream produces an `AssistantMessageEvent::Error`, the loop classifies the `error_message` string into a `AgentError` variant via pattern matching in `classify_stream_error` (`src/loop_.rs`):

| Pattern in `error_message` (case-insensitive) | AgentError variant |
|---|---|
| `"context window"` or `"context_length_exceeded"` | `ContextWindowOverflow` |
| `"rate limit"`, `"429"`, or `"throttl"` | `ModelThrottled` |
| _(stop_reason is Aborted)_ | `Aborted` |
| _(anything else)_ | `StreamError` |

This classification determines whether the error is retryable (`ModelThrottled` triggers the retry strategy), triggers overflow recovery (`ContextWindowOverflow`), or is treated as a non-retryable failure (`StreamError`).

> **StreamError vs in-message errors.** `StreamError` is a non-retryable `AgentError` produced when the `StreamFn` itself fails or when `classify_stream_error` cannot match the error to a more specific variant. In-message errors are a distinct path: the provider returns an `AssistantMessageEvent::Error` event that the loop captures and classifies. If the classified result is non-retryable, the loop builds an `AssistantMessage` with `stop_reason: Error` and emits it as a `MessageEnd` agent event. The two paths share the `StreamError` variant name but originate from different failure points.

---

## L3 — AgentError Taxonomy

```mermaid
flowchart LR
    subgraph AgentError["AgentError (enum)"]
        CWO["ContextWindowOverflow<br/>{ model: String }"]
        ModelThrottled["ModelThrottled"]
        NetErr["NetworkError<br/>{ source: Box&lt;dyn Error&gt; }"]
        StructFail["StructuredOutputFailed<br/>{ attempts: usize, last_error: String }"]
        AlreadyRunning["AlreadyRunning"]
        NoMessages["NoMessages<br/>(continue with empty context)"]
        InvalidContinue["InvalidContinue<br/>(last message is assistant)"]
        StreamError["StreamError<br/>{ source: Box&lt;dyn Error&gt; }"]
        Aborted["Aborted"]
        Plugin["Plugin<br/>{ name: String, source: Box&lt;dyn Error&gt; }"]
        BudgetExceeded["BudgetExceeded<br/>{ BudgetExceeded }"]
    end

    subgraph Trigger["Triggered by…"]
        T1["provider rejects — context too large"]
        T2["rate limit / 429 from provider"]
        T3["transient IO / connection failure"]
        T4["structured output max retries exceeded"]
        T5["prompt() called while running"]
        T6["continue() with zero messages"]
        T7["continue() from assistant message"]
        T8["StreamFn non-retryable failure"]
        T9["CancellationToken cancelled"]
        T10["plugin or extension failure"]
        T11["eval gate cost/turn budget exceeded"]
    end

    CWO --- T1
    ModelThrottled --- T2
    NetErr --- T3
    StructFail --- T4
    AlreadyRunning --- T5
    NoMessages --- T6
    InvalidContinue --- T7
    StreamError --- T8
    Aborted --- T9
    Plugin --- T10
    BudgetExceeded --- T11

    classDef errStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef trigStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000

    class CWO,ModelThrottled,NetErr,StructFail,AlreadyRunning,NoMessages,InvalidContinue,StreamError,Aborted,Plugin,BudgetExceeded errStyle
    class T1,T2,T3,T4,T5,T6,T7,T8,T9,T10,T11 trigStyle
```

---

## L3 — RetryStrategy Trait

```mermaid
flowchart TB
    subgraph Trait["RetryStrategy (trait)"]
        ShouldRetry["should_retry(<br/>  error: &AgentError,<br/>  attempt: u32<br/>) → bool"]
        Delay["delay(<br/>  attempt: u32<br/>) → Duration"]
    end

    subgraph Default["DefaultRetryStrategy"]
        MaxAttempts["max_attempts: u32 (default 3)"]
        BaseDelay["base_delay: Duration (default 1s)"]
        MaxDelay["max_delay: Duration (default 60s)"]
        Multiplier["multiplier: f64 (default 2.0)"]
        Jitter["jitter: bool (default true)"]
        RetryOn["retries on: ModelThrottled, NetworkError"]
        NeverOn["never retries: ContextWindowOverflow,<br/>Aborted, AlreadyRunning, StructuredOutputFailed,<br/>Plugin, NoMessages, InvalidContinue, StreamError,<br/>BudgetExceeded"]
    end

    ShouldRetry --> Default
    Delay --> Default
    MaxAttempts --> Delay
    BaseDelay --> Delay
    MaxDelay --> Delay
    Multiplier --> Delay
    Jitter --> Delay

    classDef traitStyle fill:#ff9800,stroke:#e65100,stroke-width:2px,color:#000
    classDef defaultStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000

    class ShouldRetry,Delay traitStyle
    class MaxAttempts,BaseDelay,MaxDelay,Multiplier,Jitter,RetryOn,NeverOn defaultStyle
```

---

## L4 — Context Window Overflow Recovery Flow

```mermaid
sequenceDiagram
    participant Caller as Caller
    participant RunLoop as run_loop
    participant Hook as transform_context hook
    participant StreamFn as StreamFn
    participant LLM as LLM Provider

    RunLoop->>Hook: transform_context(messages, overflow=false)
    Hook-->>RunLoop: messages (unchanged or lightly pruned)
    RunLoop->>StreamFn: call stream()
    StreamFn->>LLM: POST inference request
    LLM-->>StreamFn: 400 / context_length_exceeded
    StreamFn-->>RunLoop: AgentError::ContextWindowOverflow

    Note over RunLoop: does NOT append error message to history
    Note over RunLoop: history is intact — caller can reduce and retry

    RunLoop-->>Caller: Err(ContextWindowOverflow)

    Note over Caller: caller decides to retry
    Caller->>RunLoop: agent.continue_loop()
    RunLoop->>Hook: transform_context(messages, overflow=true)
    Note over Hook: overflow signal triggers more aggressive pruning
    Hook-->>RunLoop: reduced messages
    RunLoop->>StreamFn: call stream()
    StreamFn->>LLM: POST inference request (smaller context)
    LLM-->>StreamFn: 200 OK — stream begins
```

---

## L4 — Max Tokens Recovery Flow

> **Note:** This recovery is handled internally by the loop. `MaxTokensReached` is not surfaced as a `AgentError` to the caller.

```mermaid
sequenceDiagram
    participant LLM as LLM Provider
    participant Stream as StreamFn
    participant RunLoop as run_loop

    LLM-->>Stream: … ToolCallDelta (partial JSON) …
    LLM-->>Stream: Done(stop_reason=length, usage)
    Stream-->>RunLoop: MessageEnd (AssistantMessage, stop_reason=length)

    Note over RunLoop: detect stop_reason == length

    RunLoop->>RunLoop: inspect content blocks for incomplete ToolCalls
    Note over RunLoop: ToolCall "search" has partial_json — arguments incomplete

    RunLoop->>RunLoop: replace incomplete ToolCall with error ToolResultMessage:<br/>"tool call incomplete — max output tokens reached"

    Note over RunLoop: context now has valid tool use / tool result pair
    RunLoop->>RunLoop: emit TurnEnd
    RunLoop->>Stream: call StreamFn for next turn
    Note over RunLoop: LLM receives coherent history and can continue
```
