# Public API Contract: Multi-Agent System

**Feature**: 009-multi-agent-system | **Date**: 2026-03-20

## AgentId

```rust
// Construction (crate-internal only)
AgentId::next() -> AgentId          // pub(crate), monotonic atomic counter

// Traits
impl Display for AgentId             // "AgentId(N)"
impl Debug, Clone, Copy, PartialEq, Eq, Hash
```

## AgentRef

```rust
pub type AgentRef = Arc<tokio::sync::Mutex<Agent>>;
```

## AgentRegistry

```rust
// Constructor
AgentRegistry::new() -> AgentRegistry
AgentRegistry::default() -> AgentRegistry

// Registration
registry.register(name, agent) -> AgentRef     // replaces on duplicate name

// Lookup
registry.get(name) -> Option<AgentRef>

// Removal
registry.remove(name) -> Option<AgentRef>

// Inspection
registry.names() -> Vec<String>
registry.len() -> usize
registry.is_empty() -> bool
```

## AgentMailbox

```rust
// Constructor
AgentMailbox::new() -> AgentMailbox
AgentMailbox::default() -> AgentMailbox

// Send
mailbox.send(message: AgentMessage)             // non-blocking push

// Receive
mailbox.drain() -> Vec<AgentMessage>            // takes all, leaves empty

// Inspection
mailbox.has_messages() -> bool
mailbox.len() -> usize
mailbox.is_empty() -> bool
```

## send_to (free function)

```rust
pub async fn send_to(
    registry: &AgentRegistry,
    agent_name: &str,
    message: AgentMessage,
) -> Result<(), AgentError>
```

Looks up agent by name, acquires lock, calls `steer(message)`. Returns `AgentError::Plugin` if agent not found.

## SubAgent

```rust
// Constructors
SubAgent::new(name, label, description) -> SubAgent
SubAgent::simple(name, label, description, system_prompt, model, stream_fn) -> SubAgent

// Builder methods (all return Self)
.with_schema(schema: Value) -> Self
.with_requires_approval(requires: bool) -> Self
.with_options(factory: impl Fn() -> AgentOptions) -> Self
.with_map_result(f: impl Fn(AgentResult) -> AgentToolResult) -> Self
```

### AgentTool implementation

```rust
fn name() -> &str
fn label() -> &str
fn description() -> &str
fn parameters_schema() -> &Value
fn requires_approval() -> bool
fn execute(tool_call_id, params, cancellation_token, on_update) -> Future<AgentToolResult>
```

Default parameter schema:
```json
{
  "type": "object",
  "properties": {
    "prompt": { "type": "string", "description": "The prompt to send to the sub-agent" }
  },
  "required": ["prompt"]
}
```

## AgentOrchestrator

```rust
// Constructor
AgentOrchestrator::new() -> AgentOrchestrator
AgentOrchestrator::default() -> AgentOrchestrator

// Builder methods
.with_supervisor(policy: impl SupervisorPolicy) -> Self
.with_channel_buffer(size: usize) -> Self
.with_max_restarts(max: u32) -> Self

// Agent registration
orchestrator.add_agent(name, options_factory: impl Fn() -> AgentOptions)
orchestrator.add_child(name, parent, options_factory: impl Fn() -> AgentOptions)

// Hierarchy inspection
orchestrator.parent_of(name) -> Option<&str>
orchestrator.children_of(name) -> Option<&[String]>
orchestrator.names() -> Vec<&str>
orchestrator.contains(name) -> bool

// Spawning
orchestrator.spawn(name) -> Result<OrchestratedHandle, AgentError>
```

## OrchestratedHandle

```rust
// Identity
handle.name() -> &str

// Messaging
handle.send_message(text) -> Result<AgentResult, AgentError>       // async
handle.send_messages(messages) -> Result<AgentResult, AgentError>  // async

// Lifecycle
handle.await_result() -> Result<AgentResult, AgentError>           // async, consumes self
handle.cancel()
handle.status() -> AgentStatus
handle.is_done() -> bool
```

## AgentRequest

```rust
pub struct AgentRequest {
    pub messages: Vec<AgentMessage>,
    pub reply: oneshot::Sender<Result<AgentResult, AgentError>>,
}
```

## SupervisorPolicy (trait)

```rust
pub trait SupervisorPolicy: Send + Sync {
    fn on_agent_error(&self, name: &str, error: &AgentError) -> SupervisorAction;
}
```

## DefaultSupervisor

```rust
DefaultSupervisor::new(max_restarts: u32) -> DefaultSupervisor
DefaultSupervisor::default() -> DefaultSupervisor     // max_restarts = 3
supervisor.max_restarts() -> u32
```

## SupervisorAction

| Variant | Description |
|---------|-------------|
| `Restart` | Restart agent with fresh state |
| `Stop` | Stop agent permanently |
| `Escalate` | Report error, keep agent alive |

## AgentStatus

| Variant | Description |
|---------|-------------|
| `Running` | Agent task is executing |
| `Completed` | Completed successfully |
| `Failed` | Failed with an error |
| `Cancelled` | Cancelled via token |

## Error Variants Used

| Error | Trigger |
|-------|---------|
| `AgentError::Plugin { source, context }` | Agent not found in registry (messaging), agent not registered (orchestrator), channel closed |
| `AgentError::Aborted` | Agent cancelled via CancellationToken |
| `AgentError::Stream(err)` | JoinHandle panic in orchestrator |
