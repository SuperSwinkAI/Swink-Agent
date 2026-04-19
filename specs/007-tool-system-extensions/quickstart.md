# Quickstart: Tool System Extensions

**Feature**: 007-tool-system-extensions | **Date**: 2026-03-20

## Build & Test

```bash
# Build the workspace
cargo build --workspace

# Build with hot-reload feature
cargo build -p swink-agent --features hot-reload

# Run all tests (includes tool system tests)
cargo test --workspace

# Verify built-in tools can be excluded
cargo test -p swink-agent --no-default-features

# Test macros crate
cargo test -p swink-agent-macros

# Lint (zero warnings policy)
cargo clippy --workspace -- -D warnings
```

## Usage Examples

### Create a Tool from a Closure (FnTool)

```rust
use schemars::JsonSchema;
use serde::Deserialize;
use swink_agent::{AgentToolResult, FnTool};

#[derive(Deserialize, JsonSchema)]
struct WeatherParams {
    city: String,
}

let tool = FnTool::new("get_weather", "Weather", "Get weather for a city.")
    .with_schema_for::<WeatherParams>()
    .with_execute_async(|params, _cancel| async move {
        let city = params["city"].as_str().unwrap_or("unknown");
        AgentToolResult::text(format!("72F in {city}"))
    });
```

### Register a Tool Call Transformer

```rust
use swink_agent::ToolCallTransformer;

// Closure-based transformer (blanket impl)
let transformer = |tool_name: &str, args: &mut serde_json::Value| {
    if tool_name == "bash" {
        // Inject a sandbox prefix
        if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
            args["command"] = serde_json::Value::String(format!("sandbox {cmd}"));
        }
    }
};

// Use in AgentLoopConfig:
// config.tool_call_transformer = Some(Box::new(transformer));
```

### Register a Tool Validator

```rust
use swink_agent::ToolValidator;

// Closure-based validator (blanket impl)
let validator = |tool_name: &str, args: &serde_json::Value| -> Result<(), String> {
    if tool_name == "bash" {
        if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
            if cmd.contains("rm -rf") {
                return Err("destructive commands are not allowed".to_string());
            }
        }
    }
    Ok(())
};

// Use in AgentLoopConfig:
// config.tool_validator = Some(Box::new(validator));
```

### Wrap a Tool with Middleware

```rust
use std::sync::Arc;
use std::time::Duration;
use swink_agent::{AgentTool, AgentToolResult, BashTool, ToolMiddleware};

let bash = Arc::new(BashTool::new());

// Timeout middleware
let with_timeout = ToolMiddleware::with_timeout(bash.clone(), Duration::from_secs(60));

// Logging middleware
let with_logging = ToolMiddleware::with_logging(bash.clone(), |name, id, is_start| {
    if is_start {
        println!("[START] {name} ({id})");
    } else {
        println!("[END]   {name} ({id})");
    }
});

// Custom middleware
let custom = ToolMiddleware::new(bash, |inner, id, params, cancel, on_update| {
    Box::pin(async move {
        println!("before execute");
        let result = inner.execute(&id, params, cancel, on_update).await;
        println!("after execute");
        result
    })
});
```

### Configure Tool Execution Policy

```rust
use std::sync::Arc;
use swink_agent::{ToolCallSummary, ToolExecutionPolicy};

// Default: concurrent (all tool calls run in parallel)
let policy = ToolExecutionPolicy::Concurrent;

// Sequential: one at a time
let policy = ToolExecutionPolicy::Sequential;

// Priority: higher values execute first, same priority runs concurrently
let policy = ToolExecutionPolicy::Priority(Arc::new(|summary: &ToolCallSummary<'_>| {
    match summary.name {
        "write_file" => 10,  // writes first
        "read_file" => 5,    // reads second
        _ => 0,              // everything else last
    }
}));
```

### Use Built-in Tools

```rust
use swink_agent::{BashTool, ReadFileTool, WriteFileTool, builtin_tools};

// Individual tools
let bash = BashTool::new();
let read = ReadFileTool::new();
let write = WriteFileTool::new();

// All built-in tools at once (Vec<Arc<dyn AgentTool>>)
let tools = builtin_tools();
```

## Dispatch Pipeline Order

The tool dispatch pipeline is fixed and executes in this order for every tool call:

1. **Approval** — `ApprovalMode` + approval callback determine if the call proceeds
2. **Transformer** — `ToolCallTransformer::transform()` rewrites arguments in place
3. **Validator** — `ToolValidator::validate()` accepts or rejects
4. **Schema Validation** — `validate_tool_arguments()` checks arguments against JSON Schema
5. **Execute** — `AgentTool::execute()` runs the tool

If any step rejects the call, subsequent steps are skipped and an error result is returned.

### Auto-Schema from Rust Types (proc macro)

```rust
use swink_agent_macros::ToolSchema;
use swink_agent::ToolParameters;

/// Parameters for the weather tool.
#[derive(ToolSchema)]
struct WeatherParams {
    /// The city to get weather for.
    city: String,
    /// Temperature units (celsius or fahrenheit).
    units: Option<String>,
}

let schema = WeatherParams::json_schema();
// Produces: {"type": "object", "properties": {"city": {"type": "string", "description": "The city to get weather for."}, "units": {"type": "string", "description": "Temperature units (celsius or fahrenheit)."}}, "required": ["city"]}
```

### Tool from Function (attribute macro)

```rust
use swink_agent_macros::tool;
use swink_agent::AgentToolResult;

#[tool(name = "weather", description = "Get weather for a city")]
async fn get_weather(city: String, units: Option<String>) -> AgentToolResult {
    let u = units.unwrap_or_else(|| "fahrenheit".into());
    AgentToolResult::text(format!("72°F in {city} ({u})"))
}

// Use the generated tool:
let tool = GetWeatherTool;
// tool.name() == "weather"
// tool.parameters_schema() has city (required) and units (optional)
```

### Tool Filtering at Registration Time

```rust
use swink_agent::tool_filter::{ToolFilter, ToolPattern};

// Allow only read-related tools
let filter = ToolFilter::new()
    .with_allowed(vec![ToolPattern::parse("read_*")]);

// Block dangerous tools, allow everything else
let filter = ToolFilter::new()
    .with_rejected(vec![
        ToolPattern::parse("bash"),
        ToolPattern::parse("^exec_.*$"),  // regex
    ]);

// Apply to a tool list
let filtered = filter.filter_tools(tools);
```

### Hot-Reload Tools from Directory

```rust
use swink_agent::hot_reload::ToolWatcher;

// Watch a directory for tool definition files
let watcher = ToolWatcher::new("./tools/")
    .with_filter(ToolFilter::new().with_rejected(vec![ToolPattern::parse("bash")]));

// Start watching — updates agent tools on file changes
let handle = watcher.start(&agent);
```

Example tool definition (`tools/list_files.toml`):
```toml
name = "list_files"
description = "List files in a directory"
command = "ls -la {path}"
requires_approval = false

[parameters]
path = { type = "string", description = "Directory path" }
```

### Noop Tool for Session Compatibility

```rust
use swink_agent::NoopTool;

// Auto-injected by the session loader, but can be created manually:
let noop = NoopTool::new("deprecated_tool");
// noop.execute(...) returns AgentToolResult::error("Tool 'deprecated_tool' is no longer available...")
```

### Tool Confirmation Payloads

```rust
use swink_agent::{AgentTool, AgentToolResult};

struct WriteFileTool;

impl AgentTool for WriteFileTool {
    // ... name, description, schema, execute ...

    fn approval_context(&self, params: &serde_json::Value) -> Option<serde_json::Value> {
        // Provide a diff preview for the approval UI
        let path = params.get("path")?.as_str()?;
        let content = params.get("content")?.as_str()?;
        Some(serde_json::json!({
            "preview_type": "file_write",
            "path": path,
            "content_length": content.len(),
            "first_100_chars": &content[..content.len().min(100)],
        }))
    }
}
```
