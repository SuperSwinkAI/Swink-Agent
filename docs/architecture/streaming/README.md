# Streaming Interface

**Source files:** `src/stream.rs`, `src/proxy.rs`
**Related:** [PRD §7](../../planning/PRD.md#7-streaming-interface)

The streaming interface is the single boundary between the harness and LLM providers. The harness never holds provider credentials or SDK clients. All inference flows through a caller-supplied `StreamFn` implementation. The built-in `ProxyStreamFn` covers environments where a server-side proxy handles auth and routing.

---

## L2 — Components

```mermaid
flowchart TB
    subgraph CallerLayer["👤 Caller"]
        CallerStreamFn["Custom StreamFn<br/>(direct provider SDK)"]
    end

    subgraph StreamLayer["📡 Streaming Interface"]
        StreamFnTrait["StreamFn (trait)<br/>stream(model, context, options)<br/>→ Stream&lt;AssistantMessageEvent&gt;"]
        StreamOptions["StreamOptions<br/>temperature · max_tokens<br/>session_id · transport"]
        EventTypes["AssistantMessageEvent<br/>(start/delta/end protocol)"]
        Delta["AssistantMessageDelta<br/>TextDelta · ThinkingDelta · ToolCallDelta"]
    end

    subgraph ProxyLayer["🔀 Proxy StreamFn"]
        ProxyStreamFn["ProxyStreamFn<br/>(built-in)"]
        SSEParser["SSE Parser<br/>(eventsource-stream)"]
        Reconstructor["Message Reconstructor<br/>(delta → partial AssistantMessage)"]
    end

    subgraph ExternalLayer["🌐 External"]
        DirectProvider["LLM Provider API<br/>(direct)"]
        ProxyServer["LLM Proxy Server<br/>(HTTP/SSE)"]
        BackendProvider["LLM Provider API<br/>(via proxy)"]
    end

    CallerStreamFn -->|"implements"| StreamFnTrait
    ProxyStreamFn -->|"implements"| StreamFnTrait
    StreamFnTrait --> StreamOptions
    StreamFnTrait --> EventTypes
    EventTypes --> Delta
    ProxyStreamFn --> SSEParser
    SSEParser --> Reconstructor
    Reconstructor --> EventTypes
    CallerStreamFn -->|"direct calls"| DirectProvider
    ProxyStreamFn -->|"POST /api/stream<br/>Bearer token"| ProxyServer
    ProxyServer -->|"proxied request"| BackendProvider

    classDef callerStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef streamStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef proxyStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef externalStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000

    class CallerStreamFn callerStyle
    class StreamFnTrait,StreamOptions,EventTypes,Delta streamStyle
    class ProxyStreamFn,SSEParser,Reconstructor proxyStyle
    class DirectProvider,ProxyServer,BackendProvider externalStyle
```

---

## L3 — AssistantMessageEvent Protocol

Events follow a strict start/delta/end protocol per content block. Each block has a `content_index` that identifies its position in the final message's content vec.

```mermaid
flowchart LR
    subgraph StreamEvents["AssistantMessageEvent variants"]
        Start["Start<br/>(stream open)"]

        subgraph TextBlock["Text block lifecycle"]
            TextStart["TextStart(content_index)"]
            TextDelta["TextDelta(content_index, delta: String)"]
            TextEnd["TextEnd(content_index)"]
        end

        subgraph ThinkingBlock["Thinking block lifecycle"]
            ThinkStart["ThinkingStart(content_index)"]
            ThinkDelta["ThinkingDelta(content_index, delta: String)"]
            ThinkEnd["ThinkingEnd(content_index, signature: Option&lt;String&gt;)"]
        end

        subgraph ToolBlock["Tool call block lifecycle"]
            ToolStart["ToolCallStart(content_index, id, name)"]
            ToolDelta["ToolCallDelta(content_index, json_fragment: String)"]
            ToolEnd["ToolCallEnd(content_index)"]
        end

        Done["Done(stop_reason, usage)"]
        Error["Error(stop_reason, error_message, partial_usage)"]
    end

    Start --> TextBlock
    Start --> ThinkingBlock
    Start --> ToolBlock
    TextBlock --> Done
    ThinkingBlock --> Done
    ToolBlock --> Done
    TextBlock --> Error
    ThinkingBlock --> Error
    ToolBlock --> Error

    classDef eventStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef termStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000

    class Start,TextStart,TextDelta,TextEnd,ThinkStart,ThinkDelta,ThinkEnd,ToolStart,ToolDelta,ToolEnd eventStyle
    class Done,Error termStyle
```

---

## L3 — ProxyStreamFn Architecture

The proxy strips the full partial message from delta events to reduce bandwidth. The client reconstructs it locally by accumulating deltas into a `partial: AssistantMessage`.

```mermaid
flowchart TB
    subgraph ProxyServer["🖥️ Proxy Server (external)"]
        ServerRecv["Receive POST /api/stream"]
        ServerAuth["Verify Bearer token"]
        ServerForward["Forward to LLM Provider"]
        ServerSSE["Stream SSE response<br/>(partial field stripped)"]
    end

    subgraph ProxyClient["🔀 ProxyStreamFn (harness)"]
        HTTPPost["POST /api/stream<br/>(model + context + options)"]
        SSERead["Read SSE stream<br/>(eventsource-stream)"]
        ParseEvent["Parse ProxyEvent JSON"]
        Accumulate["Accumulate into<br/>partial: AssistantMessage"]
        EmitEvent["Emit AssistantMessageEvent<br/>(with partial attached)"]
    end

    subgraph Output["📤 Output"]
        HarnessStream["Stream&lt;AssistantMessageEvent&gt;<br/>consumed by run_loop"]
    end

    HTTPPost --> ServerRecv
    ServerRecv --> ServerAuth
    ServerAuth --> ServerForward
    ServerForward --> ServerSSE
    ServerSSE --> SSERead
    SSERead --> ParseEvent
    ParseEvent --> Accumulate
    Accumulate --> EmitEvent
    EmitEvent --> HarnessStream

    classDef serverStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000
    classDef clientStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef outputStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000

    class ServerRecv,ServerAuth,ServerForward,ServerSSE serverStyle
    class HTTPPost,SSERead,ParseEvent,Accumulate,EmitEvent clientStyle
    class HarnessStream outputStyle
```

---

## L4 — Delta Accumulation Sequence

This sequence shows how the harness reconstructs a complete `AssistantMessage` from individual delta events, including a text block and a tool call block arriving in the same stream.

```mermaid
sequenceDiagram
    participant Provider as LLM Provider / Proxy
    participant Stream as ProxyStreamFn / StreamFn
    participant Loop as run_loop

    Provider-->>Stream: Start
    Stream->>Loop: emit MessageStart (empty AssistantMessage)

    Provider-->>Stream: TextStart(index=0)
    Provider-->>Stream: TextDelta(index=0, "Hello")
    Provider-->>Stream: TextDelta(index=0, " world")
    Provider-->>Stream: TextEnd(index=0)
    Stream->>Loop: emit MessageUpdate(TextDelta×3)

    Provider-->>Stream: ToolCallStart(index=1, id="c1", name="search")
    Provider-->>Stream: ToolCallDelta(index=1, '{"q":')
    Provider-->>Stream: ToolCallDelta(index=1, '"rust"}')
    Provider-->>Stream: ToolCallEnd(index=1)
    Stream->>Loop: emit MessageUpdate(ToolCallDelta×2)

    Provider-->>Stream: Done(stop_reason=tool_use, usage={…})
    Stream->>Loop: emit MessageEnd (finalised AssistantMessage)
    Note over Loop: message.content = [Text("Hello world"), ToolCall("search", {q:"rust"})]
```

---

## L4 — Proxy Error Handling

Proxy failures are classified into `HarnessError` variants based on the nature of the failure. This determines whether the harness will retry the request (via `RetryStrategy`) or surface the error immediately to the caller.

| Failure mode | HarnessError variant | Retryable? | Notes |
|---|---|---|---|
| **Connection failure** (proxy unreachable, DNS failure, TCP timeout) | `HarnessError::NetworkError` | Yes | Retryable via `RetryStrategy`. |
| **Authentication failure** (invalid/expired bearer token, 401/403 response) | `HarnessError::StreamError` | No | Not retryable — caller must fix credentials. |
| **SSE stream drop** (connection lost mid-stream) | `HarnessError::NetworkError` | Yes | The harness does not attempt partial message recovery — the entire turn is retried. |
| **Proxy timeout** (proxy returns 504 or similar gateway timeout) | `HarnessError::NetworkError` | Yes | Retryable via `RetryStrategy`. |
| **Malformed SSE event** (unparseable JSON in event data) | `HarnessError::StreamError` | No | Not retryable — indicates a proxy bug. |
| **Rate limiting from proxy** (429 response from the proxy itself) | `HarnessError::ModelThrottled` | Yes | Retryable via `RetryStrategy`. |

```mermaid
flowchart TB
    subgraph Failures["🔴 Proxy Failure Modes"]
        ConnFail["Connection Failure<br/>(unreachable, DNS, TCP timeout)"]
        AuthFail["Auth Failure<br/>(401 / 403)"]
        StreamDrop["SSE Stream Drop<br/>(mid-stream disconnect)"]
        ProxyTimeout["Proxy Timeout<br/>(504 gateway timeout)"]
        Malformed["Malformed Event<br/>(unparseable JSON)"]
        RateLimit["Rate Limited<br/>(429 from proxy)"]
    end

    subgraph ErrorTypes["⚠️ HarnessError Mapping"]
        NetErr["NetworkError<br/>(retryable)"]
        StreamErr["StreamError<br/>(not retryable)"]
        Throttled["ModelThrottled<br/>(retryable)"]
    end

    ConnFail --> NetErr
    StreamDrop --> NetErr
    ProxyTimeout --> NetErr
    AuthFail --> StreamErr
    Malformed --> StreamErr
    RateLimit --> Throttled

    classDef failStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef errStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000

    class ConnFail,AuthFail,StreamDrop,ProxyTimeout,Malformed,RateLimit failStyle
    class NetErr,StreamErr,Throttled errStyle
```
