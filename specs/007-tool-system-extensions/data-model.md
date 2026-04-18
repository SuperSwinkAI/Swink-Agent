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
| `approval_context` | `&self, params: &Value -> Option<Value>` | Rich context for approval UI (default: `None`) |
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

**Builders**: `new(name, label, description)`, `with_schema_for::<T>()`, `with_schema(Value)`, `with_requires_approval(bool)`, `with_execute(closure)`, `with_execute_simple(closure)`, `with_execute_typed::<T>(closure)`

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
| `context` | `Option<Value>` | Rich context from `approval_context()` for the approval UI |

---

### ApprovalMode (enum)

Controls whether the approval gate is active.

| Variant | Description |
|---|---|
| `Enabled` | Every tool call goes through approval callback (default) |
| `Smart` | Auto-approve tools where `requires_approval()` is false |
| `Bypassed` | All tool calls auto-approved |

---

### ToolParameters (trait) — from `swink-agent-macros`

Trait implemented by `#[derive(ToolSchema)]`. Provides a static method to generate JSON Schema.

| Method | Signature | Description |
|---|---|---|
| `json_schema` | `() -> Value` | Returns JSON Schema for the struct's fields |

---

### ToolSchema (derive macro) — `swink-agent-macros` crate

Proc macro that generates a `ToolParameters` implementation. Maps:
- Field names → property names
- `String` → `{"type": "string"}`
- `u64`/`i64`/`u32`/`i32`/`usize`/`isize` → `{"type": "integer"}`
- `f64`/`f32` → `{"type": "number"}`
- `bool` → `{"type": "boolean"}`
- `Option<T>` → type of `T`, field omitted from `required`
- `Vec<T>` → `{"type": "array", "items": <T>}`
- `///` doc comments → `"description"` field
- `#[tool(description = "...")]` → overrides doc comment description

---

### #[tool] (attribute macro) — `swink-agent-macros` crate

Attribute macro that wraps an async function as an `AgentTool` implementation. Accepts `name` and `description` attributes.

```rust
#[tool(name = "weather", description = "Get weather for a city")]
async fn get_weather(city: String, units: Option<String>) -> AgentToolResult { ... }
```

Generates: a struct (e.g., `GetWeatherTool`), a `ToolParameters` impl for the parameters, and an `AgentTool` impl that deserializes parameters and calls the original function.

---

### ToolWatcher (struct, feature-gated: `hot-reload`)

Monitors a directory for tool definition file changes and updates an agent's tool list.

| Field | Type | Description |
|---|---|---|
| `watch_dir` | `PathBuf` | Directory to monitor |
| `tools` | `Arc<Mutex<Vec<Arc<dyn AgentTool>>>>` | Current loaded tools |
| `filter` | `Option<ToolFilter>` | Optional filter applied to loaded tools |

**Constructors**: `new(watch_dir: impl Into<PathBuf>)`, `with_filter(self, ToolFilter) -> Self`

**Methods**:
- `start(&self, agent: &Agent) -> JoinHandle<()>` — begins watching, updates agent on changes
- `stop(&self)` — stops the watcher

Uses `notify` crate for filesystem events. Debounces rapid changes.

---

### ScriptTool (struct, feature-gated: `hot-reload`)

A tool loaded from a TOML/YAML/JSON definition file that executes a shell command.

| Field | Type | Description |
|---|---|---|
| `name` | `String` | Tool name from definition |
| `description` | `String` | Tool description from definition |
| `command` | `String` | Shell command template |
| `schema` | `Value` | JSON Schema for parameters |
| `requires_approval` | `bool` | Whether approval is required (default: true) |

**Definition file format** (TOML example):
```toml
name = "list_files"
description = "List files in a directory"
command = "ls -la {path}"
requires_approval = false

[parameters]
path = { type = "string", description = "Directory path" }
```

Implements `AgentTool`. The `execute()` method interpolates parameters into the command template and runs it via `sh -c`.

---

### ToolFilter (struct)

Pattern-based tool filtering applied at registration time.

| Field | Type | Description |
|---|---|---|
| `allowed` | `Vec<ToolPattern>` | Patterns for allowed tool names (empty = allow all) |
| `rejected` | `Vec<ToolPattern>` | Patterns for rejected tool names (takes precedence) |

**Constructors**: `new()`, `with_allowed(patterns)`, `with_rejected(patterns)`

**Methods**:
- `matches(&self, tool_name: &str) -> bool` — returns `true` if the tool should be included
- `filter_tools(&self, tools: Vec<Arc<dyn AgentTool>>) -> Vec<Arc<dyn AgentTool>>` — filters a tool list

Implements: `Debug`, `Clone`, `Default` (default allows all).

---

### ToolPattern (enum)

A pattern for matching tool names.

| Variant | Data | Description |
|---|---|---|
| `Exact` | `String` | Exact string match |
| `Glob` | `String` | Glob pattern (e.g., `read_*`) |
| `Regex` | `Regex` | Compiled regex pattern |

**Constructor**: `ToolPattern::parse(s: &str) -> Self` — auto-detects: if contains `*` or `?` → Glob, if starts with `^` or ends with `$` → Regex, else Exact.

Implements: `Debug`, `Clone`.

---

### NoopTool (struct)

Placeholder tool for session history compatibility.

| Field | Type | Description |
|---|---|---|
| `name` | `String` | Name of the missing tool |
| `schema` | `Value` | Empty object schema |

**Constructor**: `new(name: impl Into<String>)`

Implements `AgentTool`:
- `name()` → stored name
- `description()` → `"This tool is no longer available."`
- `requires_approval()` → `false`
- `execute()` → `AgentToolResult::error("Tool '{name}' is no longer available...")`

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

ToolFilter → contains Vec<ToolPattern> for allowed/rejected
ToolFilter → applied at Agent::set_tools() / tool registration
ToolWatcher → monitors directory → loads ScriptTool definitions → updates Agent tools
ToolWatcher → optionally uses ToolFilter to restrict loaded tools
ScriptTool → implements AgentTool → executes shell commands from definition files
NoopTool → implements AgentTool → injected by session loader for missing tools
ToolParameters → trait generated by #[derive(ToolSchema)] → returns JSON Schema Value
#[tool] macro → generates struct + AgentTool impl from async fn
ToolApprovalRequest.context → populated from AgentTool::approval_context()
```
