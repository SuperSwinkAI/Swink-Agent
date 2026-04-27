# Quickstart: JSON-RPC Agent Service

## Start the Daemon

```sh
# Build and run with the cli feature
cargo run -p swink-agent-rpc --features cli --bin swink-agentd -- \
  --listen /tmp/swink.sock \
  --model claude-sonnet-4-6 \
  --system-prompt "You are a helpful assistant."

# With --force to remove a stale socket
cargo run -p swink-agent-rpc --features cli --bin swink-agentd -- \
  --listen /tmp/swink.sock --force
```

## Connect a Client (Rust)

```rust
use swink_agent_rpc::AgentClient;

let mut client = AgentClient::connect("/tmp/swink.sock").await?;
let events = client.prompt_text("Hello!").await?;
println!("{} events received", events.len());
client.shutdown().await?;
```

## Connect a Client with Tool Approval

```rust
use swink_agent_rpc::AgentClient;
use swink_agent::{ToolApproval, ToolApprovalRequest};

let mut client = AgentClient::connect("/tmp/swink.sock")
    .await?
    .with_approval_handler(|req: ToolApprovalRequest| {
        if req.tool_name == "dangerous_tool" {
            ToolApproval::Rejected
        } else {
            ToolApproval::Approved
        }
    });

let events = client.prompt_text("Run some tools").await?;
```

## Drive the Protocol by Hand

```sh
# Connect with netcat and send raw JSON-RPC
nc -U /tmp/swink.sock

# Send initialize
{"jsonrpc":"2.0","method":"initialize","params":{"protocol_version":"1.0","client":{"name":"nc","version":"1.0"}}}

# Wait for initialized response, then send a prompt
{"jsonrpc":"2.0","id":1,"method":"prompt","params":{"text":"Hello"}}

# Observe agent.event notifications streaming back
# Send shutdown when done
{"jsonrpc":"2.0","method":"shutdown","params":null}
```

## Run Tests

```sh
# Unit tests (JSON-RPC peer)
cargo test -p swink-agent-rpc --test peer

# All tests
cargo test -p swink-agent-rpc
```
