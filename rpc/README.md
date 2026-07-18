# swink-agent-rpc

JSON-RPC 2.0 service crate for exposing a `swink_agent::Agent` over an
NDJSON transport.

The primary transport is a Unix-domain socket with single-session access,
peer-credential checks, prompt streaming, cancellation, shutdown, and tool
approval forwarding. Non-Unix platforms compile descriptive stubs so the
workspace remains portable while the wire transport stays Unix-only.

Protocol 1.1 adds control-plane requests so a remote client can steer the
agent between turns: `model.list` / `model.set`, `thinking.set`,
`approval.get` / `approval.set`, `system_prompt.set`, `agent.reset`,
`plan.enter` / `plan.exit`, and `session.snapshot` / `session.restore`.
While a turn is in flight these are answered with `RpcError::BUSY`; the
`cancel` notification remains the mid-turn-safe way to abort a turn. See the
crate docs for the full method table and DTO shapes.

## Features

- `client` (default): remote agent client API.
- `server` (default): Unix socket server API.
- `cli`: `swink-agentd` daemon binary and adapter-backed model setup.

