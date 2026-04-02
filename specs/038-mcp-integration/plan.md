# Implementation Plan: MCP Integration

**Branch**: `038-mcp-integration` | **Date**: 2026-04-01 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/038-mcp-integration/spec.md`

## Summary

Add Model Context Protocol (MCP) client support to Swink Agent, enabling agents to dynamically discover and invoke tools from external MCP servers at runtime. Implemented as a new workspace crate (`swink-agent-mcp`) using the `rmcp` crate for protocol handling, with stdio and SSE transports. MCP tools implement `AgentTool` and are indistinguishable from native tools to the LLM and policy system.

## Technical Context

**Language/Version**: Rust 1.88 (edition 2024)
**Primary Dependencies**: `rmcp` (official MCP SDK — stdio, SSE, tool discovery, tool invocation), `swink-agent` (core types: `AgentTool`, `AgentToolResult`, `ContentBlock`, `AgentEvent`), `tokio` (async runtime, subprocess management), `serde`/`serde_json` (serialization), `thiserror` (errors), `tracing` (diagnostics)
**Storage**: N/A (in-memory state only — connection handles and discovered tool lists)
**Testing**: `cargo test --workspace` — mock MCP servers via `rmcp` test utilities or in-process stdio pipes
**Target Platform**: Any platform supporting tokio (Linux, macOS, Windows)
**Project Type**: Library crate (workspace member)
**Performance Goals**: <50ms overhead per MCP tool call (stdio), <2s tool discovery at startup
**Constraints**: Feature-gated (`mcp`), zero cost when disabled, no unsafe code
**Scale/Scope**: Support up to 10 concurrent MCP server connections per agent

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Notes |
|-----------|--------|-------|
| I. Library-First | PASS | New workspace crate `swink-agent-mcp`, depends only on `swink-agent` public API. Self-contained, independently compilable and testable. |
| II. Test-Driven Development | PASS | Tests use mock MCP servers (in-process stdio pipes). No external services required. |
| III. Efficiency & Performance | PASS | Async tool calls via tokio. No allocations on hot paths beyond JSON serialization (unavoidable for MCP protocol). |
| IV. Leverage the Ecosystem | PASS | Uses `rmcp` (official MCP SDK by MCP org) rather than hand-rolling protocol. |
| V. Provider Agnosticism | PASS | MCP is a tool protocol, not a provider. Does not touch `StreamFn` or LLM communication. Tools implement `AgentTool` trait. |
| VI. Safety & Correctness | PASS | `#[forbid(unsafe_code)]`. MCP errors → `AgentToolResult::error()`, never panics. |

**Architectural Constraints Check**:

| Constraint | Status | Notes |
|------------|--------|-------|
| Crate count (currently 10 members) | JUSTIFY | Adding 11th member. Justified: MCP is a distinct protocol concern with its own dependency (`rmcp`) that should not pollute core, adapters, or policies. No existing crate boundary can absorb this — it's not a provider adapter (adapters handle LLM streaming), not a policy, not memory/eval/TUI. |
| MSRV 1.88 | PASS | `rmcp` supports current stable Rust. |
| Concurrency model | PASS | MCP tool calls run concurrently via existing `tokio::spawn` tool dispatch. Connection management uses tokio tasks. |
| Events outward-only | PASS | MCP emits `AgentEvent` variants. No re-entrant state mutation. |
| No global mutable state | PASS | Connection state in `Arc<Mutex<>>` per server, owned by the MCP manager. |

## Project Structure

### Documentation (this feature)

```text
specs/038-mcp-integration/
├── plan.md              # This file
├── research.md          # Phase 0 output
├── data-model.md        # Phase 1 output
├── quickstart.md        # Phase 1 output
├── contracts/           # Phase 1 output
└── tasks.md             # Phase 2 output (/speckit.tasks)
```

### Source Code (repository root)

```text
mcp/
├── Cargo.toml           # swink-agent-mcp crate
├── src/
│   ├── lib.rs           # Public API re-exports, #[forbid(unsafe_code)]
│   ├── config.rs        # McpServerConfig, McpTransport, ToolFilter
│   ├── connection.rs    # McpConnection — wraps rmcp client session
│   ├── manager.rs       # McpManager — multi-server orchestration
│   ├── tool.rs          # McpTool — AgentTool impl for discovered tools
│   ├── convert.rs       # rmcp types ↔ swink-agent types conversion
│   ├── event.rs         # MCP-specific AgentEvent emission helpers
│   └── error.rs         # McpError type
└── tests/
    ├── common/
    │   └── mod.rs        # Mock MCP server helpers
    ├── connection_test.rs
    ├── manager_test.rs
    ├── tool_test.rs
    └── filter_test.rs

src/
└── loop_/
    └── event.rs          # Add MCP event variants to AgentEvent
```

**Structure Decision**: New crate `mcp/` follows the established pattern (adapters, policies, auth, memory). Core crate gains new `AgentEvent` variants only. No changes to `AgentTool` trait — MCP tools implement the existing trait.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| 11th workspace crate | MCP brings `rmcp` dependency (~15 transitive deps) that would pollute any existing crate. It's a distinct protocol concern (tool discovery + execution over JSON-RPC) orthogonal to LLM adapters, policies, memory, and eval. | Putting MCP in adapters was considered — rejected because adapters implement `StreamFn` for LLM providers, while MCP implements `AgentTool` for tool servers. Different trait, different concern. Putting in core would force `rmcp` on all consumers. |
