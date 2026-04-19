# Public API: Tool System Extensions

**Feature**: 007-tool-system-extensions | **Date**: 2026-03-20

All types are re-exported from `swink_agent` via `lib.rs`. Consumers never reach into submodules.

## `src/tool.rs` — Core Trait and Types

```rust
/// A tool that can be invoked by the agent loop.
pub trait AgentTool: Send + Sync {
    fn name(&self) -> &str;
    fn label(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> &Value;
    fn requires_approval(&self) -> bool { false }
    fn metadata(&self) -> Option<ToolMetadata> { None }
    fn approval_context(&self, _params: &Value) -> Option<Value> { None }
    fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        cancellation_token: CancellationToken,
        on_update: Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>,
    ) -> Pin<Box<dyn Future<Output = AgentToolResult> + Send + '_>>;
}

#[derive(Debug, Clone)]
pub struct AgentToolResult {
    pub content: Vec<ContentBlock>,
    pub details: Value,
    pub is_error: bool,
}

impl AgentToolResult {
    pub fn text(text: impl Into<String>) -> Self;
    pub fn error(message: impl Into<String>) -> Self;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolMetadata {
    pub namespace: Option<String>,
    pub version: Option<String>,
}

impl ToolMetadata {
    pub fn with_namespace(namespace: impl Into<String>) -> Self;
    pub fn with_version(mut self, version: impl Into<String>) -> Self;
}

pub fn validate_schema(schema: &Value) -> Result<(), String>;
pub fn validate_tool_arguments(schema: &Value, arguments: &Value) -> Result<(), Vec<String>>;
pub fn unknown_tool_result(tool_name: &str) -> AgentToolResult;
pub fn validation_error_result(errors: &[String]) -> AgentToolResult;
pub fn redact_sensitive_values(value: &Value) -> Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolApproval {
    Approved,
    Rejected,
    ApprovedWith(Value),
}

#[derive(Clone)]
pub struct ToolApprovalRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub requires_approval: bool,
    pub context: Option<Value>,  // from AgentTool::approval_context()
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApprovalMode {
    #[default]
    Enabled,
    Smart,
    Bypassed,
}

pub fn selective_approve<F>(inner: F) -> Box<dyn Fn(ToolApprovalRequest) -> Pin<Box<dyn Future<Output = ToolApproval> + Send>> + Send + Sync>
where
    F: Fn(ToolApprovalRequest) -> Pin<Box<dyn Future<Output = ToolApproval> + Send>> + Send + Sync + 'static;
```

## `src/tool_call_transformer.rs` — SUPERSEDED by 031 PreDispatchPolicy

> **Removed by [031-policy-slots](../../031-policy-slots/spec.md).** Argument rewriting is now handled by PreDispatchPolicy implementations in Slot 2, which receive `&mut arguments` in ToolPolicyContext.

```rust
// REMOVED — replaced by PreDispatchPolicy (031)
// pub trait ToolCallTransformer: Send + Sync {
//     fn transform(&self, tool_name: &str, arguments: &mut Value);
// }
```

## `src/tool_validator.rs` — SUPERSEDED by 031 PreDispatchPolicy

> **Removed by [031-policy-slots](../../031-policy-slots/spec.md).** Tool call rejection is now handled by PreDispatchPolicy implementations returning PolicyVerdict::Skip.

```rust
// REMOVED — replaced by PreDispatchPolicy (031)
// pub trait ToolValidator: Send + Sync {
//     fn validate(&self, tool_name: &str, arguments: &Value) -> Result<(), String>;
// }
```

## `src/tool_middleware.rs` — Execution Decorator

```rust
/// Intercepts execute() on a wrapped AgentTool.
/// All metadata methods delegate to the inner tool.
pub struct ToolMiddleware { /* inner: Arc<dyn AgentTool>, middleware_fn: Arc<MiddlewareFn> */ }

impl ToolMiddleware {
    pub fn new<F>(inner: Arc<dyn AgentTool>, f: F) -> Self
    where
        F: Fn(Arc<dyn AgentTool>, String, Value, CancellationToken, Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>)
            -> Pin<Box<dyn Future<Output = AgentToolResult> + Send>>
            + Send + Sync + 'static;

    pub fn with_timeout(inner: Arc<dyn AgentTool>, timeout: Duration) -> Self;

    pub fn with_logging<F>(inner: Arc<dyn AgentTool>, callback: F) -> Self
    where
        F: Fn(&str, &str, bool) + Send + Sync + 'static;
}

impl AgentTool for ToolMiddleware { ... }  // delegates metadata, intercepts execute
```

## `src/tool_execution_policy.rs` — Dispatch Ordering

```rust
/// Lightweight borrowed view of a pending tool call.
#[derive(Debug)]
pub struct ToolCallSummary<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub arguments: &'a Value,
}

/// Callback assigning priority to a tool call. Higher values execute first.
pub type PriorityFn = dyn Fn(&ToolCallSummary<'_>) -> i32 + Send + Sync;

/// Fully custom tool execution strategy.
pub trait ToolExecutionStrategy: Send + Sync {
    fn partition(
        &self,
        tool_calls: &[ToolCallSummary<'_>],
    ) -> Pin<Box<dyn Future<Output = Vec<Vec<usize>>> + Send + '_>>;
}

/// Controls how tool calls within a single turn are dispatched.
#[derive(Default)]
pub enum ToolExecutionPolicy {
    #[default]
    Concurrent,
    Sequential,
    Priority(Arc<PriorityFn>),
    Custom(Arc<dyn ToolExecutionStrategy>),
}

impl Clone for ToolExecutionPolicy { ... }
impl Debug for ToolExecutionPolicy { ... }
```

## `src/fn_tool.rs` — Closure-Based Tool Builder

```rust
/// A tool built from closures, implementing AgentTool without a custom struct.
pub struct FnTool { /* name, label, description, schema, requires_approval, execute_fn */ }

impl FnTool {
    pub fn new(name: impl Into<String>, label: impl Into<String>, description: impl Into<String>) -> Self;
    pub fn with_schema_for<T: JsonSchema>(mut self) -> Self;
    pub fn with_schema(mut self, schema: Value) -> Self;
    pub const fn with_requires_approval(mut self, requires: bool) -> Self;
    pub fn with_execute<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(String, Value, CancellationToken, Option<Box<dyn Fn(AgentToolResult) + Send + Sync>>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AgentToolResult> + Send + 'static;
    pub fn with_execute_simple<F, Fut>(mut self, f: F) -> Self
    where
        F: Fn(Value, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AgentToolResult> + Send + 'static;
    pub fn with_execute_async<F, Fut>(self, f: F) -> Self
    where
        F: Fn(Value, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AgentToolResult> + Send + 'static;
    pub fn with_execute_typed<T, F, Fut>(mut self, f: F) -> Self
    where
        T: DeserializeOwned + JsonSchema + Send + 'static,
        F: Fn(T, CancellationToken) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AgentToolResult> + Send + 'static;
}

impl AgentTool for FnTool { ... }
impl Debug for FnTool { ... }
```

## `src/tools/` — Built-in Tools (feature-gated)

```rust
// All gated behind #[cfg(feature = "builtin-tools")]

pub struct BashTool { /* schema: Value */ }
impl BashTool {
    pub fn new() -> Self;
}
impl Default for BashTool { ... }
impl AgentTool for BashTool { ... }  // name: "bash", requires_approval: true

/// Surgical find-and-replace editing tool. Supports:
/// - Multiple edits per call, applied top-to-bottom in-memory (fail-fast)
/// - Exact match with whitespace-normalised fallback (trailing whitespace per line)
/// - replace_all flag for replacing every occurrence
/// - line_hint for disambiguating when old_string matches multiple times
/// - expected_hash (SHA-256 hex) for stale-read detection
/// - Atomic write via sibling temp file + rename
pub struct EditFileTool { /* schema: Value */ }
impl EditFileTool {
    pub fn new() -> Self;
}
impl Default for EditFileTool { ... }
impl AgentTool for EditFileTool { ... }  // name: "edit_file", requires_approval: true

pub struct ReadFileTool { /* schema: Value */ }
impl ReadFileTool {
    pub fn new() -> Self;
}
impl Default for ReadFileTool { ... }
impl AgentTool for ReadFileTool { ... }  // name: "read_file", requires_approval: false

pub struct WriteFileTool { /* schema: Value */ }
impl WriteFileTool {
    pub fn new() -> Self;
}
impl Default for WriteFileTool { ... }
impl AgentTool for WriteFileTool { ... }  // name: "write_file", requires_approval: true

/// Returns all built-in tools (bash, edit_file, read_file, write_file) wrapped in Arc.
pub fn builtin_tools() -> Vec<Arc<dyn AgentTool>>;
```

## Re-exports from `lib.rs`

```rust
pub use tool::{
    AgentTool, AgentToolResult, ApprovalMode, ToolApproval, ToolApprovalRequest, ToolMetadata,
    redact_sensitive_values, selective_approve, unknown_tool_result, validate_schema,
    validate_tool_arguments, validation_error_result,
};
// [031] Superseded — replaced by PreDispatchPolicy slot:
// pub use tool_call_transformer::ToolCallTransformer;
// pub use tool_validator::ToolValidator;
pub use tool_middleware::ToolMiddleware;
pub use tool_execution_policy::{
    PriorityFn, ToolCallSummary, ToolExecutionPolicy, ToolExecutionStrategy,
};
pub use fn_tool::FnTool;

#[cfg(feature = "builtin-tools")]
pub use tools::{BashTool, EditFileTool, ReadFileTool, WriteFileTool, builtin_tools};
// EditFileTool is individually constructable: EditFileTool::new() or Default::default()

pub use tool_filter::{ToolFilter, ToolPattern};
pub use noop_tool::NoopTool;

#[cfg(feature = "hot-reload")]
pub use hot_reload::{ToolWatcher, ScriptTool};
```

## `src/tool_filter.rs` — Registration-Time Filtering

```rust
/// Pattern for matching tool names.
#[derive(Debug, Clone)]
pub enum ToolPattern {
    Exact(String),
    Glob(String),
    Regex(regex::Regex),
}

impl ToolPattern {
    /// Auto-detects pattern type: glob if contains * or ?, regex if starts with ^ or ends with $, else exact.
    pub fn parse(s: &str) -> Self;
    pub fn matches(&self, name: &str) -> bool;
}

/// Filters tools at registration time by name patterns.
#[derive(Debug, Clone, Default)]
pub struct ToolFilter {
    pub allowed: Vec<ToolPattern>,
    pub rejected: Vec<ToolPattern>,
}

impl ToolFilter {
    pub fn new() -> Self;
    pub fn with_allowed(mut self, patterns: Vec<ToolPattern>) -> Self;
    pub fn with_rejected(mut self, patterns: Vec<ToolPattern>) -> Self;
    pub fn matches(&self, tool_name: &str) -> bool;
    pub fn filter_tools(&self, tools: Vec<Arc<dyn AgentTool>>) -> Vec<Arc<dyn AgentTool>>;
}
```

## `src/noop_tool.rs` — Session History Compatibility

```rust
/// Placeholder tool for tools that no longer exist in the registry.
pub struct NoopTool {
    name: String,
    schema: Value,
}

impl NoopTool {
    pub fn new(name: impl Into<String>) -> Self;
}

impl AgentTool for NoopTool { ... }
// name() → stored name
// description() → "This tool is no longer available."
// requires_approval() → false
// execute() → AgentToolResult::error("Tool '{name}' is no longer available...")
```

## `swink-agent-macros` — Proc Macro Crate

```rust
// In swink-agent core crate (src/tool.rs or src/tool_parameters.rs):
pub trait ToolParameters {
    fn json_schema() -> Value;
}

// In swink-agent-macros crate:
#[proc_macro_derive(ToolSchema, attributes(tool))]
pub fn derive_tool_schema(input: TokenStream) -> TokenStream;

#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream;
```

## `src/hot_reload.rs` — Tool Hot-Reloading (feature-gated: `hot-reload`)

```rust
/// Tool loaded from a definition file (TOML/YAML/JSON).
pub struct ScriptTool { ... }
impl AgentTool for ScriptTool { ... }

/// Watches a directory for tool definition changes.
pub struct ToolWatcher { ... }

impl ToolWatcher {
    pub fn new(watch_dir: impl Into<PathBuf>) -> Self;
    pub fn with_filter(mut self, filter: ToolFilter) -> Self;
    pub fn start(&self, agent: &Agent) -> tokio::task::JoinHandle<()>;
    pub fn stop(&self);
}
```
