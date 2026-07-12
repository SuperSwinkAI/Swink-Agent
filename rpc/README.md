# swink-agent-rpc

JSON-RPC 2.0 service crate for exposing a `swink_agent::Agent` over an
NDJSON transport.

The primary transport is a Unix-domain socket with single-session access,
peer-credential checks, prompt streaming, cancellation, shutdown, and tool
approval forwarding. Non-Unix platforms compile descriptive stubs so the
workspace remains portable while the wire transport stays Unix-only.

## Features

- `client` (default): remote agent client API.
- `server` (default): Unix socket server API.
- `cli`: `swink-agentd` daemon binary and adapter-backed model setup.

