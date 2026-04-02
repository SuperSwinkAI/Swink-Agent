# Data Model: TransferToAgent Tool & Handoff Safety

**Feature**: 040-agent-transfer-handoff  
**Date**: 2026-04-02

## Entities

### TransferSignal

Data structure carrying all information needed for the target agent to continue the conversation.

| Field | Type | Description |
|-------|------|-------------|
| target_agent | String | Name of the agent to transfer to |
| reason | String | Why the transfer is happening |
| context_summary | Option\<String\> | Optional concise handoff brief |
| conversation_history | Vec\<AgentMessage\> | Messages to carry over (Llm variants only, custom messages filtered) |

- **Derives**: Clone, Debug, Serialize, Deserialize

### TransferToAgentTool

Tool implementation that signals handoff intent. Implements `AgentTool`.

| Field | Type | Description |
|-------|------|-------------|
| registry | Arc\<AgentRegistry\> | Reference to agent registry for target validation |
| allowed_targets | Option\<HashSet\<String\>\> | If Some, restricts which agents can be targets. None = unrestricted |

- **Tool name**: `transfer_to_agent`
- **Parameters**: `agent_name` (required string), `reason` (required string), `context_summary` (optional string)
- **Constructors**: `new(registry)`, `with_allowed_targets(registry, targets)`

### TransferChain

Safety mechanism tracking the ordered sequence of agents in a transfer chain.

| Field | Type | Description |
|-------|------|-------------|
| chain | Vec\<String\> | Ordered list of agent names in this chain |
| max_depth | usize | Maximum allowed chain depth (default: 5) |

- **Derives**: Clone, Debug
- **Methods**: `new(max_depth)`, `push(agent_name) -> Result<(), TransferError>`, `depth() -> usize`, `contains(agent_name) -> bool`

### TransferError

Error type for transfer chain safety violations.

| Variant | Fields | Description |
|---------|--------|-------------|
| CircularTransfer | agent_name: String, chain: Vec\<String\> | Agent already appears in the chain |
| MaxDepthExceeded | depth: usize, max: usize | Chain would exceed configured max depth |

- **Derives**: Debug, Clone
- **Implements**: Display, Error

## Modified Existing Types

### StopReason (modified)

New variant added:

| Variant | Fields | Description |
|---------|--------|-------------|
| Transfer | — (unit variant) | Agent loop terminated due to transfer signal |

Unit variant preserves the `Copy` derive. Transfer details live in `AgentResult.transfer_signal`.

### AgentResult (modified)

New field added:

| Field | Type | Description |
|-------|------|-------------|
| transfer_signal | Option\<TransferSignal\> | Present when `stop_reason == Transfer` |

Mirrors the `error: Option<String>` pattern alongside `StopReason::Error`.

### AgentToolResult (modified)

New field added:

| Field | Type | Description |
|-------|------|-------------|
| transfer_signal | Option\<TransferSignal\> | Present when tool result signals a transfer. None for normal results |

New constructor: `transfer(signal: TransferSignal) -> Self`

Serde: `#[serde(default, skip_serializing_if = "Option::is_none")]`

## Relationships

```
TransferToAgentTool *──1 AgentRegistry (via Arc)
TransferToAgentTool ──> AgentToolResult (with transfer_signal)
AgentLoop ──detects──> AgentToolResult.transfer_signal
AgentLoop ──enriches──> TransferSignal (adds conversation_history)
AgentLoop ──sets──> AgentResult { stop_reason: Transfer, transfer_signal: Some(...) }
Orchestrator ──owns──> TransferChain
Orchestrator ──consults──> TransferChain before dispatching transfer
```

## State Transitions

### Transfer Flow

```
AgentRunning ──[LLM calls transfer_to_agent]──> ToolExecuting
ToolExecuting ──[target valid]──> TransferSignalReturned
ToolExecuting ──[target invalid/not allowed]──> ErrorResultReturned ──> AgentRunning (loop continues)
TransferSignalReturned ──[loop detects signal]──> TurnTerminated(Transfer)
TurnTerminated ──[caller receives AgentResult]──> OrchestratorDecides
OrchestratorDecides ──[chain check passes]──> TargetAgentStarted
OrchestratorDecides ──[circular/depth error]──> TransferRejected
```
