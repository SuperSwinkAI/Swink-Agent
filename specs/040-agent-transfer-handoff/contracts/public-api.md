# Public API Contract: TransferToAgent Tool & Handoff Safety

**Feature**: 040-agent-transfer-handoff  
**Date**: 2026-04-02

## Feature Gate

| Feature | Default | Modules |
|---------|---------|---------|
| `transfer` | yes | `transfer` module (TransferToAgentTool, TransferSignal, TransferChain, TransferError) |

Note: `StopReason::Transfer` variant, `AgentResult.transfer_signal`, and `AgentToolResult.transfer_signal` are always compiled (not gated).

## New Public Types (re-exported from `lib.rs` when `transfer` feature enabled)

```
TransferToAgentTool   — AgentTool implementation for handoff signaling
TransferSignal        — Handoff payload (target, reason, summary, history)
TransferChain         — Safety mechanism for circular detection
TransferError         — Error variants (CircularTransfer, MaxDepthExceeded)
```

## Modified Public Types (always compiled)

```
StopReason::Transfer  — New unit variant (preserves Copy)
AgentResult           — New field: transfer_signal: Option<TransferSignal>
AgentToolResult       — New field: transfer_signal: Option<TransferSignal>
                        New constructor: transfer(signal) -> Self
```

## TransferToAgentTool API

```
TransferToAgentTool::new(registry: Arc<AgentRegistry>) -> Self
TransferToAgentTool::with_allowed_targets(
    registry: Arc<AgentRegistry>,
    targets: impl IntoIterator<Item = impl Into<String>>,
) -> Self
```

Implements `AgentTool`:
- `name()` -> `"transfer_to_agent"`
- `description()` -> describes transfer capability
- `schema()` -> JSON schema with `agent_name` (required), `reason` (required), `context_summary` (optional)
- `execute(args, token)` -> `AgentToolResult` with `transfer_signal: Some(...)` on success, error result on failure

## TransferSignal API

```
TransferSignal::new(
    target_agent: impl Into<String>,
    reason: impl Into<String>,
) -> Self

TransferSignal::with_context_summary(self, summary: impl Into<String>) -> Self
TransferSignal::with_conversation_history(self, history: Vec<AgentMessage>) -> Self

// Accessors
TransferSignal::target_agent(&self) -> &str
TransferSignal::reason(&self) -> &str
TransferSignal::context_summary(&self) -> Option<&str>
TransferSignal::conversation_history(&self) -> &[AgentMessage]
```

## TransferChain API

```
TransferChain::new(max_depth: usize) -> Self
TransferChain::default() -> Self   // max_depth = 5
TransferChain::push(&mut self, agent_name: impl Into<String>) -> Result<(), TransferError>
TransferChain::depth(&self) -> usize
TransferChain::contains(&self, agent_name: &str) -> bool
TransferChain::chain(&self) -> &[String]
```

## AgentToolResult Extensions

```
AgentToolResult::transfer(signal: TransferSignal) -> Self
AgentToolResult::is_transfer(&self) -> bool
```

## Trait Implementations

| Type | Traits |
|------|--------|
| TransferSignal | Clone, Debug, Serialize, Deserialize |
| TransferToAgentTool | AgentTool (Send + Sync) |
| TransferChain | Clone, Debug |
| TransferError | Debug, Clone, Display, Error |

## Tool Schema

```json
{
  "type": "object",
  "properties": {
    "agent_name": { "type": "string", "description": "Name of the agent to transfer to" },
    "reason": { "type": "string", "description": "Why the transfer is needed" },
    "context_summary": { "type": "string", "description": "Optional summary for the target agent" }
  },
  "required": ["agent_name", "reason"]
}
```
