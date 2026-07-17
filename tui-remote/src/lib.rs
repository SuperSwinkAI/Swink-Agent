//! Remote transport for the `swink-agent` TUI.
//!
//! Bridges [`swink_agent_tui`]'s [`TuiTransport`] seam to a remote agent
//! served by `swink-agentd` over JSON-RPC, via [`swink_agent_rpc::AgentClient`].
//! This crate is the only place the TUI and RPC stacks meet — `swink-agent-tui`
//! and `swink-agent-rpc` deliberately have no dependency edge between them.
//!
//! # Quick start
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use swink_agent_tui::{App, TuiConfig, setup_terminal};
//! use swink_agent_tui_remote::RemoteTransport;
//!
//! let transport = RemoteTransport::connect("/tmp/swink.sock").await?;
//! let mut app = App::new(TuiConfig::load()).with_transport(Box::new(transport));
//! let mut terminal = setup_terminal()?;
//! app.run(&mut terminal).await?;
//! # Ok(())
//! # }
//! ```
//!
//! Or run the packaged binary: `swink-tui-remote /tmp/swink.sock`.
//!
//! # Scope
//!
//! The transport carries **turn I/O only**: user input out, [`AgentEvent`]s
//! back. Control-plane operations that require an in-process
//! [`Agent`](swink_agent::Agent) — abort, model cycling, plan mode, session
//! save/load — are not available over the wire until [`TuiTransport`] grows
//! control methods.
//!
//! # Tool approval
//!
//! Without an approval handler, [`AgentClient`] auto-approves every
//! `tool.approve` request the server sends. Configure the approval policy on
//! the server (`swink-agentd`'s [`AgentOptions`](swink_agent::AgentOptions)
//! factory), or build a client with
//! [`AgentClient::with_approval_handler`] and wrap it via
//! [`RemoteTransport::from_client`].

#![forbid(unsafe_code)]

use std::path::Path;
use std::pin::Pin;

use swink_agent::AgentEvent;
use swink_agent_rpc::AgentClient;
use swink_agent_rpc::jsonrpc::RpcError;
use swink_agent_tui::{TransportError, TuiTransport, UserInput};
use tokio::sync::mpsc;

/// A [`TuiTransport`] backed by a remote agent over JSON-RPC.
///
/// A background bridge task owns the [`AgentClient`]: each [`UserInput`]
/// becomes a `prompt` request, and every [`AgentEvent`] the server streams
/// during the turn is forwarded to the TUI event loop as it arrives.
///
/// The bridge task exits when the transport is dropped (the input channel
/// closes) or when the connection to the server is lost, after which
/// [`recv`](TuiTransport::recv) yields `None`.
pub struct RemoteTransport {
    /// Send side: the TUI event loop pushes user input here.
    input_tx: mpsc::Sender<UserInput>,
    /// Receive side: the TUI event loop reads agent events here.
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,
}

impl RemoteTransport {
    /// Connect to a running `swink-agentd` at the given Unix socket path.
    ///
    /// Tool-approval requests from the server are auto-approved; see the
    /// crate docs and [`RemoteTransport::from_client`] for supplying a
    /// handler instead.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket cannot be connected or the protocol
    /// handshake fails.
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, RpcError> {
        let client = AgentClient::connect(path).await?;
        Ok(Self::from_client(client))
    }

    /// Wrap an already-connected [`AgentClient`].
    ///
    /// Use this to configure the client first — most commonly
    /// [`AgentClient::with_approval_handler`] — before handing it to the
    /// transport. Spawns the background bridge task, so this must be called
    /// from within a Tokio runtime.
    #[must_use]
    pub fn from_client(mut client: AgentClient) -> Self {
        let (input_tx, mut input_rx) = mpsc::channel::<UserInput>(64);
        let (event_tx, event_rx) = mpsc::unbounded_channel::<AgentEvent>();

        tokio::spawn(async move {
            while let Some(input) = input_rx.recv().await {
                let forward = event_tx.clone();
                let result = client
                    .prompt_text_with(input.text, |event| {
                        let _ = forward.send(event);
                    })
                    .await;

                if let Err(e) = result {
                    tracing::error!("RemoteTransport: turn failed: {e}");
                    // Surface an end-of-turn so the UI leaves its streaming
                    // state, mirroring InProcessTransport's error behavior.
                    let _ = event_tx.send(AgentEvent::AgentEnd {
                        messages: std::sync::Arc::new(Vec::new()),
                    });
                    if e.code == RpcError::DISCONNECTED {
                        return;
                    }
                }
            }
        });

        Self { input_tx, event_rx }
    }
}

impl TuiTransport for RemoteTransport {
    fn send(
        &self,
        input: UserInput,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send + '_>> {
        let tx = self.input_tx.clone();
        Box::pin(async move {
            tx.send(input)
                .await
                .map_err(|_| TransportError::channel_closed())
        })
    }

    fn recv(
        &mut self,
    ) -> Pin<Box<dyn std::future::Future<Output = Option<AgentEvent>> + Send + '_>> {
        Box::pin(async move { self.event_rx.recv().await })
    }

    fn try_recv(&mut self) -> Option<AgentEvent> {
        self.event_rx.try_recv().ok()
    }
}
