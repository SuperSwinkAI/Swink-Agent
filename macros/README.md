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
- `#[schemars(description = "...")]` and other `schemars` attributes pass through unchanged — schema customization is delegated entirely to the `schemars` SDK, this crate adds no attribute language of its own
- `#[tool(...)]` applies only to async functions; on a struct field it is a compile error (it is not a helper attribute of `#[derive(ToolSchema)]`)

## Scope: external SDK consumers

This crate targets **downstream users of the `swink-agent` SDK** who want to turn a plain async function into a tool with minimal ceremony. The built-in tools inside the `swink-agent` crate itself intentionally hand-roll the `AgentTool` trait and do not use these macros:

- The macro expansion names the SDK by its external path (`swink_agent::…`), which is not resolvable from within the `swink-agent` crate itself.
- `#[tool]` generates a stateless unit struct whose `label()` equals its `name()`; the built-ins need constructor state (artifact stores, execution roots), human-readable labels, `deny_unknown_fields` schemas, and `execution_root`/`approval_context` overrides — capabilities the macro deliberately does not model.

To keep the macros from silently drifting away from the real trait, the integration tests in [`tests/`](tests/) and the runnable example [`examples/derived_tool.rs`](examples/derived_tool.rs) both compile and execute the generated tools against the actual `swink-agent` crate:

```sh
cargo run -p swink-agent-macros --example derived_tool
```

## Quick Start

```toml
[dependencies]
swink-agent = "0.9.0"
swink-agent-macros = "0.9.0"
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
