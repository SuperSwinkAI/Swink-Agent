# Data Model: Tool System Extensions

**Feature**: 007-tool-system-extensions | **Date**: 2026-03-20

## Entities

### AgentTool (trait)

The core trait that all tools implement. Object-safe, `Send + Sync`.

| Method | Signature | Description |
|---|---|---|
| `name` | `&self -> &str` | Unique routing key for dispatch |
| `label` | `&self -> &str` | Human-readable display name |
| `description` | `&self -> &str` | Natural-language description for LLM prompt |
| `parameters_schema` | `&self -> &Value` | JSON Schema for input validation |
| `requires_approval` | `&self -> bool` | Whether approval gate applies (default: `false`) |
| `metadata` | `&self -> Option<ToolMetadata>` | Optional namespace/version (default: `None`) |
| `execute` | `&self, &str, Value, CancellationToken, Option<Box<dyn Fn(AgentToolResult) + Send + Sync>> -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>>` | Execute with validated params |

---

### AgentToolResult (struct)

| Field | Type | Description |
|---|---|---|
| `content` | `Vec<ContentBlock>` | Content blocks returned to the LLM |
| `details` | `Value` | Structured data for logging (not sent to LLM) |
| `is_error` | `bool` | Whether this result represents an error |

**Constructors**: `text(impl Into<String>)`, `error(impl Into<String>)`

---

### ToolMetadata (struct)

| Field | Type | Description |
|---|---|---|
| `namespace` | `Option<String>` | Logical grouping (e.g., "filesystem") |
| `version` | `Option<String>` | Semver version string |

**Constructors**: `with_namespace(impl Into<String>)`, builder `with_version(self, impl Into<String>)`

---

### ToolCallTransformer (trait) — superseded by 031 PreDispatchPolicy

Pre-validation argument rewriting hook. Synchronous. Runs unconditionally (not gated by approval).

| Method | Signature | Description |
|---|---|---|
| `transform` | `&self, tool_name: &str, arguments: &mut Value` | Mutate arguments in place |

**Blanket impl**: `Fn(&str, &mut Value) + Send + Sync`

---

### ToolValidator (trait) — superseded by 031 PreDispatchPolicy

Post-transformation validation hook. Synchronous. Rejects with error message or accepts.

| Method | Signature | Description |
|---|---|---|
| `validate` | `&self, tool_name: &str, arguments: &Value -> Result<(), String>` | Accept or reject |

**Blanket impl**: `Fn(&str, &Value) -> Result<(), String> + Send + Sync`

---

### ToolMiddleware (struct)

Decorator wrapping an `AgentTool`'s `execute()` method. Delegates all metadata methods to the inner tool. Implements `AgentTool`.

| Field | Type | Description |
|---|---|---|
| `inner` | `Arc<dyn AgentTool>` | The wrapped tool |
| `middleware_fn` | `Arc<MiddlewareFn>` | Closure intercepting `execute()` |

**Type alias**: `MiddlewareFn = dyn Fn(Arc<dyn AgentTool>, String, Value, CancellationToken, Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send>> + Send + Sync`

**Constructors**:
- `new(inner, closure)` — custom middleware
- `with_timeout(inner, Duration)` — timeout enforcement
- `with_logging(inner, callback)` — before/after logging

---

### ToolExecutionPolicy (enum)

Controls how tool calls within a single turn are dispatched.

| Variant | Data | Description |
|---|---|---|
| `Concurrent` | — | All tool calls run concurrently via `tokio::spawn` (default) |
| `Sequential` | — | Tool calls run one at a time in order |
| `Priority` | `Arc<PriorityFn>` | Groups by priority; concurrent within group, sequential across groups |
| `Custom` | `Arc<dyn ToolExecutionStrategy>` | Fully custom partitioning |

**Type alias**: `PriorityFn = dyn Fn(&ToolCallSummary<'_>) -> i32 + Send + Sync`

Implements `Clone`, `Debug`, `Default` (defaults to `Concurrent`).

---

### ToolCallSummary (struct, borrowed)

Lightweight borrowed view of a pending tool call, used by policy callbacks.

| Field | Type | Description |
|---|---|---|
| `id` | `&'a str` | Unique tool call identifier |
| `name` | `&'a str` | Tool name |
| `arguments` | `&'a Value` | Tool call arguments |

---

### ToolExecutionStrategy (trait)

Fully custom execution strategy for advanced dispatch ordering.

| Method | Signature | Description |
|---|---|---|
| `partition` | `&self, &[ToolCallSummary<'_>] -> Pin<Box<dyn Future<Output = Vec<Vec<usize>>> + Send + '_>>` | Return groups of indices; concurrent within group, sequential across groups |

---

### FnTool (struct)

Closure-based tool builder. Implements `AgentTool`.

| Field | Type | Description |
|---|---|---|
| `name` | `String` | Tool routing key |
| `label` | `String` | Display name |
| `description` | `String` | LLM prompt description |
| `schema` | `Value` | JSON Schema for parameters |
| `requires_approval` | `bool` | Approval gate flag |
| `execute_fn` | `Arc<ExecuteFn>` | Stored execution closure |

**Type alias**: `ExecuteFn = dyn Fn(String, Value, CancellationToken, Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send>> + Send + Sync`

**Builders**: `new(name, label, description)`, `with_schema_for::<T>()`, `with_schema(Value)`, `with_requires_approval(bool)`, `with_execute(closure)`, `with_execute_simple(closure)`

---

### BashTool (struct, feature-gated)

Built-in shell command execution tool. Requires approval.

| Field | Type | Description |
|---|---|---|
| `schema` | `Value` | Pre-computed JSON Schema |

**Parameters** (JSON Schema):
- `command: String` — Shell command to execute
- `timeout_ms: Option<u64>` — Timeout in milliseconds (default 30000)

---

### ReadFileTool (struct, feature-gated)

Built-in file reading tool. Does not require approval.

| Field | Type | Description |
|---|---|---|
| `schema` | `Value` | Pre-computed JSON Schema |

**Parameters** (JSON Schema):
- `path: String` — Absolute path to the file to read

---

### WriteFileTool (struct, feature-gated)

Built-in file writing tool. Requires approval.

| Field | Type | Description |
|---|---|---|
| `schema` | `Value` | Pre-computed JSON Schema |

**Parameters** (JSON Schema):
- `path: String` — Absolute path to write
- `content: String` — Content to write to the file

---

### ToolApproval (enum)

Result of the approval gate for a tool call.

| Variant | Data | Description |
|---|---|---|
| `Approved` | — | Tool call proceeds |
| `Rejected` | — | Tool call blocked |
| `ApprovedWith` | `Value` | Approved with modified parameters |

---

### ToolApprovalRequest (struct)

Information about a tool call pending approval. Debug impl redacts `arguments`.

| Field | Type | Description |
|---|---|---|
| `tool_call_id` | `String` | Unique call ID |
| `tool_name` | `String` | Tool being called |
| `arguments` | `Value` | Arguments passed to the tool |
| `requires_approval` | `bool` | Whether the tool declared approval requirement |

---

### ApprovalMode (enum)

Controls whether the approval gate is active.

| Variant | Description |
|---|---|
| `Enabled` | Every tool call goes through approval callback (default) |
| `Smart` | Auto-approve tools where `requires_approval()` is false |
| `Bypassed` | All tool calls auto-approved |

---

## Relationships

```text
AgentLoopConfig
├── tools: Vec<Arc<dyn AgentTool>>          # Includes FnTool, ToolMiddleware, BashTool, etc.
├── pre_dispatch_policies: Vec<Arc<dyn PreDispatchPolicy>>  # [031] replaces tool_call_transformer + tool_validator
├── tool_execution_policy: ToolExecutionPolicy
├── approval_callback: Option<ApprovalFn>
└── approval_mode: ApprovalMode

Dispatch Pipeline (fixed order) — [Updated by 031]:
  PreDispatchPolicy::evaluate() (Slot 2, may transform args or Skip)
    → ApprovalMode + approval_callback
      → validate_tool_arguments() (schema, hardcoded)
        → AgentTool::execute()

ToolMiddleware → wraps Arc<dyn AgentTool> → delegates metadata, intercepts execute()
FnTool → implements AgentTool via stored closures
BashTool / ReadFileTool / WriteFileTool → implement AgentTool directly

ToolExecutionPolicy::Priority → uses PriorityFn(ToolCallSummary)
ToolExecutionPolicy::Custom → uses ToolExecutionStrategy::partition(ToolCallSummary[])
```
