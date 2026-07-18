# swink-agent-tui-remote

Remote transport for the [swink-agent](https://github.com/SuperSwinkAI/Swink-Agent) TUI:
drive the stock terminal UI against an agent served by `swink-agentd` over JSON-RPC.

This crate is the bridge between `swink-agent-tui` (which defines the
`TuiTransport` seam) and `swink-agent-rpc` (which speaks the wire protocol).
The two stacks have no dependency edge between them; this crate is the only
place they meet.

## Binary

```sh
# On the server machine (or another terminal):
swink-agentd --socket /tmp/swink.sock

# Attach the TUI:
swink-tui-remote /tmp/swink.sock
```

## Library

```rust,no_run
# async fn run() -> Result<(), Box<dyn std::error::Error>> {
use swink_agent_tui::{App, TuiConfig, setup_terminal};
use swink_agent_tui_remote::RemoteTransport;

let transport = RemoteTransport::connect("/tmp/swink.sock").await?;
let mut app = App::new(TuiConfig::load()).with_transport(Box::new(transport));
let mut terminal = setup_terminal()?;
app.run(&mut terminal).await?;
# Ok(())
# }
```

## Scope

The transport carries turn I/O (user input out, `AgentEvent`s back) and the
`TuiTransport` control plane: abort, model listing/cycling, thinking level,
approval mode, system prompt, reset, plan mode, and session snapshot/restore
(protocol 1.1). Control requests queue in order with prompts; abort is sent
out-of-band as a `cancel` notification so it works mid-turn.

Session snapshots use the memory-JSONL wire representation. Custom messages
are skipped with a warning on the client side (no `CustomMessageRegistry`
over the wire).

Tool approval is decided server-side: without a client approval handler,
`tool.approve` requests are auto-approved. Configure the approval mode in the
`swink-agentd` agent factory, or wrap a pre-configured
`AgentClient::with_approval_handler` via `RemoteTransport::from_client`.
