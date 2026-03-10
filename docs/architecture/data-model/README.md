# Data Model

**Source file:** `src/types.rs`
**Related:** [PRD §3](../../planning/PRD.md#3-core-data-model)

The data model defines every type that crosses a public boundary in the harness. All other modules depend on it; it depends on nothing else in the crate.

---

## L2 — Type Groups

The types are organised into five cohesive groups. Every other module in the harness imports from one or more of these groups.

```mermaid
flowchart TB
    subgraph ContentLayer["📄 Content"]
        ContentBlock["ContentBlock<br/>Text · Thinking · ToolCall · Image"]
    end

    subgraph MessageLayer["💬 Messages"]
        UserMessage["UserMessage"]
        AssistantMessage["AssistantMessage"]
        ToolResultMessage["ToolResultMessage"]
        LlmMessage["LlmMessage<br/>(enum wrapping the three above)"]
        AgentMessage["AgentMessage<br/>(LlmMessage | Custom)"]
    end

    subgraph ModelLayer["🤖 Model"]
        ModelSpec["ModelSpec<br/>provider · model_id · thinking_level · budgets"]
        ThinkingLevel["ThinkingLevel<br/>Off · Minimal · Low · Medium · High · XHigh"]
        ThinkingBudgets["ThinkingBudgets<br/>per-level token overrides"]
    end

    subgraph UsageLayer["📊 Usage"]
        Usage["Usage<br/>input · output · cache_read · cache_write · total"]
        Cost["Cost<br/>per-category + total (f64)"]
    end

    subgraph ResultLayer["✅ Results"]
        AgentResult["AgentResult<br/>messages · stop_reason · usage · error"]
        StopReason["StopReason<br/>stop · length · tool_use · aborted · error"]
    end

    ContentBlock --> UserMessage
    ContentBlock --> AssistantMessage
    ContentBlock --> ToolResultMessage
    UserMessage --> LlmMessage
    AssistantMessage --> LlmMessage
    ToolResultMessage --> LlmMessage
    LlmMessage --> AgentMessage
    Usage --> AssistantMessage
    Cost --> Usage
    StopReason --> AssistantMessage
    StopReason --> AgentResult
    Usage --> AgentResult
    AgentMessage --> AgentResult
    ThinkingLevel --> ModelSpec
    ThinkingBudgets --> ModelSpec

    classDef contentStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef messageStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef modelStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef usageStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000
    classDef resultStyle fill:#e0e0e0,stroke:#424242,stroke-width:2px,color:#000

    class ContentBlock contentStyle
    class UserMessage,AssistantMessage,ToolResultMessage,LlmMessage,AgentMessage messageStyle
    class ModelSpec,ThinkingLevel,ThinkingBudgets modelStyle
    class Usage,Cost usageStyle
    class AgentResult,StopReason resultStyle
```

---

## L3 — Message Type Composition

Each message type is a distinct struct. `LlmMessage` is a discriminated enum wrapping all three. `AgentMessage` extends this further with an open custom variant.

```mermaid
flowchart TB
    subgraph UserMsg["UserMessage"]
        UM_role["role: user"]
        UM_content["content: Vec&lt;ContentBlock&gt;<br/>(Text | Image only)"]
        UM_ts["timestamp: u64"]
    end

    subgraph AssistantMsg["AssistantMessage"]
        AM_role["role: assistant"]
        AM_content["content: Vec&lt;ContentBlock&gt;<br/>(Text | Thinking | ToolCall)"]
        AM_provider["provider: String"]
        AM_model["model_id: String"]
        AM_usage["usage: Usage"]
        AM_stop["stop_reason: StopReason"]
        AM_err["error_message: Option&lt;String&gt;"]
        AM_ts["timestamp: u64"]
    end

    subgraph ToolResultMsg["ToolResultMessage"]
        TR_role["role: tool_result"]
        TR_id["tool_call_id: String"]
        TR_content["content: Vec&lt;ContentBlock&gt;<br/>(Text | Image only)"]
        TR_err["is_error: bool"]
        TR_ts["timestamp: u64"]
    end

    subgraph LlmMsg["LlmMessage (enum)"]
        LLM_user["User(UserMessage)"]
        LLM_asst["Assistant(AssistantMessage)"]
        LLM_tool["ToolResult(ToolResultMessage)"]
    end

    subgraph AgentMsg["AgentMessage (enum)"]
        AM_llm["Llm(LlmMessage)"]
        AM_custom["Custom(Box&lt;dyn CustomMessage&gt;)"]
    end

    UserMsg --> LLM_user
    AssistantMsg --> LLM_asst
    ToolResultMsg --> LLM_tool
    LLM_user --> AM_llm
    LLM_asst --> AM_llm
    LLM_tool --> AM_llm
    AM_llm --> AgentMsg
    AM_custom --> AgentMsg

    classDef userStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000
    classDef assistStyle fill:#1976d2,stroke:#0d47a1,stroke-width:2px,color:#fff
    classDef toolStyle fill:#ff9800,stroke:#e65100,stroke-width:2px,color:#000
    classDef enumStyle fill:#f5f5f5,stroke:#616161,stroke-width:2px,color:#000
    classDef fieldStyle fill:#fafafa,stroke:#bdbdbd,stroke-width:1px,color:#000

    class UM_role,UM_content,UM_ts fieldStyle
    class AM_role,AM_content,AM_provider,AM_model,AM_usage,AM_stop,AM_err,AM_ts fieldStyle
    class TR_role,TR_id,TR_content,TR_err,TR_ts fieldStyle
    class LLM_user,LLM_asst,LLM_tool enumStyle
    class AM_llm,AM_custom enumStyle
```

---

## L3 — ContentBlock Variants

`ContentBlock` is the atomic unit of all message content. Different variants are permitted in different message roles.

```mermaid
flowchart LR
    subgraph Variants["ContentBlock variants"]
        Text["Text<br/>text: String"]
        Thinking["Thinking<br/>thinking: String<br/>signature: Option&lt;String&gt;"]
        ToolCall["ToolCall<br/>id: String<br/>name: String<br/>arguments: Value<br/>partial_json: Option&lt;String&gt;"]
        Image["Image<br/>source: ImageSource"]
    end

    subgraph Permitted["Permitted in…"]
        InUser["UserMessage"]
        InAssistant["AssistantMessage"]
        InToolResult["ToolResultMessage"]
    end

    Text -->|"✓"| InUser
    Text -->|"✓"| InAssistant
    Text -->|"✓"| InToolResult
    Thinking -->|"✓"| InAssistant
    ToolCall -->|"✓"| InAssistant
    Image -->|"✓"| InUser
    Image -->|"✓"| InToolResult

    classDef blockStyle fill:#fff3e0,stroke:#f57c00,stroke-width:2px,color:#000
    classDef msgStyle fill:#e3f2fd,stroke:#1976d2,stroke-width:2px,color:#000

    class Text,Thinking,ToolCall,Image blockStyle
    class InUser,InAssistant,InToolResult msgStyle
```

---

## L4 — AgentMessage Lifetime

This sequence shows how an `AgentMessage` is created, mutated during streaming, and finalised within a single turn.

```mermaid
sequenceDiagram
    participant Loop as Agent Loop
    participant Stream as StreamFn
    participant Msg as AgentMessage

    Loop->>Stream: call StreamFn
    Stream-->>Loop: AssistantMessageEvent::Start
    Loop->>Msg: create AssistantMessage (empty content)

    loop streaming deltas
        Stream-->>Loop: AssistantMessageEvent::Delta(TextDelta | ThinkingDelta | ToolCallDelta)
        Loop->>Msg: append fragment to ContentBlock
    end

    Stream-->>Loop: AssistantMessageEvent::Done(usage, stop_reason)
    Loop->>Msg: set usage, stop_reason, timestamp
    Note over Msg: Message is now immutable
    Loop->>Msg: wrap in AgentMessage::Llm(LlmMessage::Assistant)
    Loop->>Loop: push to context.messages
```
