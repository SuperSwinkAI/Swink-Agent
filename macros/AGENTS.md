# AGENTS.md — swink-agent-macros

## Scope

`macros/` — Proc macros for tool authoring. Proc-macro crate; no runtime deps.

## Key Facts

- `#[derive(ToolSchema)]` — generates a `ToolParameters` impl that delegates to `schemars`. The struct must also derive `JsonSchema` (re-exported as `swink_agent::JsonSchema`). Doc comments are picked up as field descriptions automatically.
- `#[tool(name = "...", description = "...")]` — wraps an async function as an `AgentTool` impl. Schema is derived from a hidden params struct via `schemars`. `name` is required; `description` is optional.
- Both macros live in `tool_schema.rs` and `tool_attr.rs` respectively.

## Lessons Learned

- `#[tool]` macro param decoding must return `AgentToolResult::error("invalid parameters: ...")` on serde failures rather than panicking. The generated code retries deserialization from `{}` so zero-param / all-optional tools can execute when appropriate.
- Schema generation runs at compile time via schemars — any type used as a tool parameter must implement `JsonSchema`. Avoid `serde_json::Value` parameters; prefer typed structs so the LLM receives a proper schema.

## Build & Test

```bash
cargo build -p swink-agent-macros
cargo test -p swink-agent-macros
cargo clippy -p swink-agent-macros -- -D warnings
```
