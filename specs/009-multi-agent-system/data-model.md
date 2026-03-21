# Data Model: Multi-Agent System

**Feature**: 009-multi-agent-system | **Date**: 2026-03-20

## Entities

### AgentId

Unique monotonic identifier assigned to every Agent on construction.

| Field | Type | Description |
|-------|------|-------------|
| (inner) | `u64` | Monotonic counter value from `AtomicU64` |

Implements: `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`, `Hash`, `Display`.

### AgentRef

Type alias for a shareable, async-safe handle to an Agent.

```rust
pub type AgentRef = Arc<tokio::sync::Mutex<Agent>>;
```

### AgentRegistry

Thread-safe registry mapping string names to `AgentRef` handles.

| Field | Type | Description |
|-------|------|-------------|
| `agents` | `Arc<RwLock<HashMap<String, AgentRef>>>` | The name-to-agent map. `std::sync::RwLock` (not tokio) because no await is held across the lock. |

Implements: `Clone`, `Default`.

### AgentMailbox

Per-agent inbox for receiving messages asynchronously.

| Field | Type | Description |
|-------|------|-------------|
| `inbox` | `Arc<Mutex<Vec<AgentMessage>>>` | Pending messages. `std::sync::Mutex` — send is a Vec push (non-blocking). |

Implements: `Clone`, `Default`.

### SubAgent

Tool wrapper that invokes a child agent, implementing `AgentTool`.

| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | Tool name exposed to the parent agent |
| `label` | `String` | Human-readable label |
| `description` | `String` | Tool description for the LLM |
| `schema` | `Value` | JSON Schema for parameters (default: `{ prompt: string }`) |
| `requires_approval` | `bool` | Whether parent must approve before execution |
| `options_factory` | `Arc<dyn Fn() -> AgentOptions + Send + Sync>` | Factory producing fresh AgentOptions per execution |
| `map_result` | `Arc<dyn Fn(AgentResult) -> AgentToolResult + Send + Sync>` | Converts agent result to tool result |

Implements: `AgentTool`, `Debug`, `Send + Sync`.

### AgentOrchestrator

Supervisor managing lifecycle, delegation, and shutdown of multiple agents.

| Field | Type | Description |
|-------|------|-------------|
| `entries` | `HashMap<String, AgentEntry>` | Registered agents with their configuration |
| `supervisor` | `Option<Arc<dyn SupervisorPolicy>>` | Error recovery policy |
| `channel_buffer` | `usize` | Request channel buffer size (default: 32) |
| `default_max_restarts` | `u32` | Default max restarts for new agents (default: 3) |

Implements: `Default`, `Debug`.

### AgentEntry (internal)

Registration info stored in the orchestrator for each agent.

| Field | Type | Description |
|-------|------|-------------|
| `options_factory` | `OptionsFactoryArc` | Factory for (re)spawning the agent |
| `parent` | `Option<String>` | Parent agent name (if child) |
| `children` | `Vec<String>` | Child agent names |
| `max_restarts` | `u32` | Max restarts allowed per spawn cycle |

### OrchestratedHandle

Handle to a spawned orchestrated agent, providing interaction and lifecycle control.

| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | Agent name |
| `request_tx` | `mpsc::Sender<AgentRequest>` | Channel for sending requests to the agent |
| `cancellation_token` | `CancellationToken` | Token for cancelling the agent |
| `join_handle` | `Option<JoinHandle<Result<AgentResult, AgentError>>>` | Tokio task handle |
| `status` | `Arc<Mutex<AgentStatus>>` | Current agent status |

Implements: `Debug`.

### AgentRequest

A message sent to a running agent via its request channel.

| Field | Type | Description |
|-------|------|-------------|
| `messages` | `Vec<AgentMessage>` | Messages to inject into the agent |
| `reply` | `oneshot::Sender<Result<AgentResult, AgentError>>` | One-shot channel for the response |

### AgentStatus

Lifecycle state of a spawned agent (defined in `handle.rs`, shared with orchestrator).

| Variant | Description |
|---------|-------------|
| `Running` | Agent task is executing |
| `Completed` | Agent task completed successfully |
| `Failed` | Agent task failed with an error |
| `Cancelled` | Agent task was cancelled |

### SupervisorPolicy (trait)

Policy that determines how to handle agent failures.

```rust
pub trait SupervisorPolicy: Send + Sync {
    fn on_agent_error(&self, name: &str, error: &AgentError) -> SupervisorAction;
}
```

### SupervisorAction

What the supervisor decides after an agent error.

| Variant | Description |
|---------|-------------|
| `Restart` | Restart the failed agent with fresh state (up to max_restarts) |
| `Stop` | Stop the agent permanently |
| `Escalate` | Report the error to the caller but keep the agent alive |

### DefaultSupervisor

Built-in supervisor that restarts on retryable errors and stops otherwise.

| Field | Type | Description |
|-------|------|-------------|
| `max_restarts` | `u32` | Maximum consecutive restarts allowed (default: 3) |

## Relationships

```
AgentRegistry --stores--> AgentRef (Arc<tokio::sync::Mutex<Agent>>)
AgentMailbox --holds--> Vec<AgentMessage>
send_to() --uses--> AgentRegistry --finds--> AgentRef --calls--> Agent::steer()

SubAgent --implements--> AgentTool
SubAgent --creates--> Agent (via options_factory)
SubAgent --propagates--> CancellationToken (parent → child)

AgentOrchestrator --manages--> AgentEntry --spawns--> OrchestratedHandle
OrchestratedHandle --sends--> AgentRequest --via--> mpsc channel --to--> Agent
OrchestratedHandle --monitors--> AgentStatus
AgentOrchestrator --applies--> SupervisorPolicy --returns--> SupervisorAction
AgentEntry --tracks--> parent/child hierarchy
```
