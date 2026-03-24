# Data Model: Configurable Policy Slots for the Agent Loop

**Feature**: 031-policy-slots | **Date**: 2026-03-24

## Entities

### PolicyVerdict (enum)

Outcome of a policy evaluation in PreTurn, PostTurn, and PostLoop slots. Does not include Skip.

| Variant | Fields | Description |
|---------|--------|-------------|
| `Continue` | — | Proceed normally |
| `Stop` | `reason: String` | Stop the loop gracefully |
| `Inject` | `messages: Vec<AgentMessage>` | Add messages to pending queue and continue |

Implements: `Debug`, `Clone`.

### PreDispatchVerdict (enum)

Outcome of a PreDispatch policy evaluation. Includes Skip for tool-call-level rejection.

| Variant | Fields | Description |
|---------|--------|-------------|
| `Continue` | — | Proceed normally |
| `Stop` | `reason: String` | Abort entire tool batch, stop the loop |
| `Inject` | `messages: Vec<AgentMessage>` | Add messages to pending queue and continue |
| `Skip` | `error_text: String` | Skip this tool call, return error text to LLM |

Implements: `Debug`, `Clone`.

### PolicyContext (struct, borrowed)

Shared read-only context available to every policy evaluation.

| Field | Type | Description |
|-------|------|-------------|
| `turn_index` | `usize` | Zero-based index of the current/completed turn |
| `accumulated_usage` | `&'a Usage` | Accumulated token usage across all turns |
| `accumulated_cost` | `&'a Cost` | Accumulated cost across all turns |
| `message_count` | `usize` | Number of messages in context |
| `overflow_signal` | `bool` | Whether context overflow was signaled |
| `new_messages` | `&'a [AgentMessage]` | Messages added since the last policy evaluation for this slot (PreTurn: pending batch; PostTurn/PostLoop/PreDispatch: empty) |

Implements: `Debug`. Lifetime `'a` borrows from loop state.

### ToolPolicyContext (struct, borrowed)

Per-tool-call context for PreDispatch policies. Provides mutable access to arguments.

| Field | Type | Description |
|-------|------|-------------|
| `tool_name` | `&'a str` | Name of the tool being called |
| `tool_call_id` | `&'a str` | Unique identifier for this tool call |
| `arguments` | `&'a mut Value` | Mutable reference to tool call arguments |

Implements: `Debug` (arguments redacted in debug output).

### TurnPolicyContext (struct, borrowed)

Per-turn context for PostTurn policies.

| Field | Type | Description |
|-------|------|-------------|
| `assistant_message` | `&'a AssistantMessage` | The assistant message from the completed turn |
| `tool_results` | `&'a [ToolResultMessage]` | Tool results produced during this turn |
| `stop_reason` | `StopReason` | Why the turn ended |

Implements: `Debug`.

### PreTurnPolicy (trait)

Slot 1: Evaluated before each LLM call. Guards and pre-conditions.

| Method | Signature | Description |
|--------|-----------|-------------|
| `name` | `&self -> &str` | Policy identifier for tracing |
| `evaluate` | `&self, ctx: &PolicyContext<'_> -> PolicyVerdict` | Evaluate the policy |

Required bounds: `Send + Sync`. (Runner uses `AssertUnwindSafe` for `catch_unwind`; implementors do not need `UnwindSafe`.)

### PreDispatchPolicy (trait)

Slot 2: Evaluated per tool call, before approval and execution. Can mutate arguments.

| Method | Signature | Description |
|--------|-----------|-------------|
| `name` | `&self -> &str` | Policy identifier for tracing |
| `evaluate` | `&self, ctx: &PolicyContext<'_>, tool: &mut ToolPolicyContext<'_> -> PreDispatchVerdict` | Evaluate the policy |

Required bounds: `Send + Sync`. (Runner uses `AssertUnwindSafe` for `catch_unwind`; implementors do not need `UnwindSafe`.)

### PostTurnPolicy (trait)

Slot 3: Evaluated after each completed turn. Reacts to turn results.

| Method | Signature | Description |
|--------|-----------|-------------|
| `name` | `&self -> &str` | Policy identifier for tracing |
| `evaluate` | `&self, ctx: &PolicyContext<'_>, turn: &TurnPolicyContext<'_> -> PolicyVerdict` | Evaluate the policy |

Required bounds: `Send + Sync`. (Runner uses `AssertUnwindSafe` for `catch_unwind`; implementors do not need `UnwindSafe`.)

### PostLoopPolicy (trait)

Slot 4: Evaluated after the inner loop exits, before follow-up polling.

| Method | Signature | Description |
|--------|-----------|-------------|
| `name` | `&self -> &str` | Policy identifier for tracing |
| `evaluate` | `&self, ctx: &PolicyContext<'_> -> PolicyVerdict` | Evaluate the policy |

Required bounds: `Send + Sync`. (Runner uses `AssertUnwindSafe` for `catch_unwind`; implementors do not need `UnwindSafe`.)

### BudgetPolicy (struct)

Built-in PreTurnPolicy. Stops when accumulated cost or tokens exceed limits.

| Field | Type | Description |
|-------|------|-------------|
| `max_cost` | `Option<f64>` | Maximum total cost |
| `max_input_tokens` | `Option<u64>` | Maximum input tokens |
| `max_output_tokens` | `Option<u64>` | Maximum output tokens |

Implements: `PreTurnPolicy`, `Debug`, `Clone`.

### MaxTurnsPolicy (struct)

Built-in PreTurnPolicy and/or PostTurnPolicy. Stops after N turns.

| Field | Type | Description |
|-------|------|-------------|
| `max_turns` | `usize` | Maximum number of turns |

Implements: `PreTurnPolicy`, `PostTurnPolicy`, `Debug`, `Clone`.

### SandboxPolicy (struct)

Built-in PreDispatchPolicy. Restricts or rewrites file paths to an allowed root.

| Field | Type | Description |
|-------|------|-------------|
| `allowed_root` | `PathBuf` | Root directory all file paths must fall within |
| `path_fields` | `Vec<String>` | Argument field names to check for paths (default: `["path", "file_path", "file"]`) |

Implements: `PreDispatchPolicy`, `Debug`, `Clone`. Behavior: Skip with error (no silent path rewriting).

### ToolDenyListPolicy (struct)

Built-in PreDispatchPolicy. Rejects tool calls by name.

| Field | Type | Description |
|-------|------|-------------|
| `denied` | `HashSet<String>` | Set of tool names to reject |

Implements: `PreDispatchPolicy`, `Debug`, `Clone`.

### CheckpointPolicy (struct)

Built-in PostTurnPolicy. Persists state after each turn.

| Field | Type | Description |
|-------|------|-------------|
| `store` | `Arc<dyn CheckpointStore>` | Checkpoint persistence backend |
| `handle` | `tokio::runtime::Handle` | Tokio runtime handle for spawning async save |

Implements: `PostTurnPolicy`, `Debug`. Persistence is fire-and-forget via `tokio::spawn`; evaluate returns Continue immediately.

### LoopDetectionPolicy (struct)

Built-in PostTurnPolicy. Detects repeated tool call patterns. Uses interior mutability (`Mutex<Vec<...>>`) to track recent turns.

| Field | Type | Description |
|-------|------|-------------|
| `lookback` | `usize` | Number of recent turns to compare |
| `on_detect` | `LoopDetectionAction` | What to do when a cycle is detected |
| `history` | `Mutex<VecDeque<Vec<(String, Value)>>>` | Recent tool call patterns (interior mutability) |

`LoopDetectionAction` enum: `Stop`, `Inject(String)` (steering message text).

Implements: `PostTurnPolicy`, `Debug`.

## Relationships

```text
AgentLoopConfig
├── pre_turn_policies: Vec<Arc<dyn PreTurnPolicy>>
├── pre_dispatch_policies: Vec<Arc<dyn PreDispatchPolicy>>
├── post_turn_policies: Vec<Arc<dyn PostTurnPolicy>>
└── post_loop_policies: Vec<Arc<dyn PostLoopPolicy>>

Slot Runner
├── run_policies(Vec<Arc<dyn {Pre,Post}*Policy>>, PolicyContext) -> PolicyVerdict
└── run_pre_dispatch_policies(Vec<Arc<dyn PreDispatchPolicy>>, PolicyContext, ToolPolicyContext) -> PreDispatchVerdict

Dispatch Pipeline (new order):
  PreDispatchPolicy::evaluate() (Slot 2, may transform args or Skip)
    → ApprovalMode + approval_callback
      → validate_tool_arguments() (schema, hardcoded)
        → AgentTool::execute()

Built-in policies:
  BudgetPolicy ──implements──> PreTurnPolicy
  MaxTurnsPolicy ──implements──> PreTurnPolicy + PostTurnPolicy
  SandboxPolicy ──implements──> PreDispatchPolicy
  ToolDenyListPolicy ──implements──> PreDispatchPolicy
  CheckpointPolicy ──implements──> PostTurnPolicy
  LoopDetectionPolicy ──implements──> PostTurnPolicy
```
