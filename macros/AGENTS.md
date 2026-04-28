# AGENTS.md — swink-agent-macros

## Scope

`macros/` — Proc macros for tool authoring. No runtime deps.

## Key Invariants

- `#[derive(ToolSchema)]` — generates `ToolParameters` impl via `schemars`. Struct must also derive `JsonSchema`. Doc comments become field descriptions.
- `#[tool(name = "...", description = "...")]` — wraps async fn as `AgentTool`. Schema derived from hidden params struct. `name` required.
- `#[tool]` param decoding returns `AgentToolResult::error(...)` on serde failures, retries from `{}` for zero-param tools.
- Tool parameter types must implement `JsonSchema`. Prefer typed structs over `serde_json::Value`.
