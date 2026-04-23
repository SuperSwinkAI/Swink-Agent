# swink-agent-macros

[![Crates.io](https://img.shields.io/crates/v/swink-agent-macros.svg)](https://crates.io/crates/swink-agent-macros)
[![Docs.rs](https://docs.rs/swink-agent-macros/badge.svg)](https://docs.rs/swink-agent-macros)
[![License: MIT](https://img.shields.io/badge/license-MIT-green.svg)](https://github.com/SuperSwinkAI/Swink-Agent/blob/main/LICENSE-MIT)

Proc macros for [`swink-agent`](https://crates.io/crates/swink-agent) — turn an async function or a params struct into a fully-typed `AgentTool` with generated JSON Schema.

## Features

- **`#[tool(name = "...", description = "...")]`** — wraps an async fn as an `AgentTool`, derives its input schema from a hidden params struct
- **`#[derive(ToolSchema)]`** — implements `ToolParameters` for any struct that also derives `schemars::JsonSchema`
- Doc comments on fields (`///`) become parameter descriptions in the generated schema
- All types supported by `schemars` are accepted — no custom primitive subset
- `#[schemars(description = "...")]` and other `schemars` attributes pass through unchanged

## Quick Start

```toml
[dependencies]
swink-agent = "0.8"
swink-agent-macros = "0.8"
schemars = "1"
serde = { version = "1", features = ["derive"] }
```

```rust,ignore
use swink_agent::JsonSchema;
use swink_agent_macros::{tool, ToolSchema};

#[derive(ToolSchema, JsonSchema, serde::Deserialize)]
struct SearchParams {
    /// The search query
    query: String,
    /// Maximum number of results
    limit: Option<u32>,
}

#[tool(name = "search", description = "Search the docs index")]
async fn search(params: SearchParams) -> Result<String, Box<dyn std::error::Error>> {
    Ok(format!("results for {:?} (limit={:?})", params.query, params.limit))
}
```

## Architecture

`#[tool]` expands to an `impl AgentTool` that wraps the async body, deserializes the tool-call arguments into the hidden params struct (derived in the same expansion), and returns the result as a `ToolResult`. Schema generation is delegated entirely to `schemars` via `swink_agent::schema_for::<Self>()` — there is no bespoke type-to-schema mapper to drift out of sync with the JSON Schema spec.

No `unsafe` code (`#![forbid(unsafe_code)]`). Macros emit only safe Rust; all unsafety boundaries are upstream in `syn`/`proc-macro2`.

---

Part of the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) workspace — see the [main README](https://github.com/SuperSwinkAI/Swink-Agent#readme) for workspace overview and setup.
