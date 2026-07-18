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
//! The transport carries turn I/O (user input out, [`AgentEvent`]s back) and
//! the [`ControlRequest`] operations: abort, model listing/switching,
//! thinking level, approval mode, system prompt, reset, plan mode, and
//! session snapshot/restore (protocol 1.1 control-plane methods).
//!
//! Control requests are queued in order with prompts, so a deferred
//! `SetModel` issued by the TUI always reaches the server before the prompt
//! that follows it. The exception is [`ControlRequest::Abort`], which is
//! sent out-of-band as a `cancel` notification — the mid-turn-safe way to
//! stop a running turn while the bridge task is still streaming it.
//!
//! Session snapshots cross the wire in the memory-JSONL representation. On
//! the client side no [`CustomMessageRegistry`](swink_agent::CustomMessageRegistry)
//! is available, so custom messages are skipped with a warning on snapshot
//! decode (the same policy as loading a session store without a registry).
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

use swink_agent::{AgentEvent, AgentMessage, LlmMessage, serialize_custom_message};
use swink_agent_rpc::AgentClient;
use swink_agent_rpc::dto::SessionSnapshot;
use swink_agent_rpc::jsonrpc::{PeerSender, RpcError};
use swink_agent_tui::{ControlRequest, ControlResponse, TransportError, TuiTransport, UserInput};
use tokio::sync::{mpsc, oneshot};

/// One unit of work for the bridge task, kept in a single queue so control
/// operations stay ordered relative to the prompts around them.
enum BridgeMsg {
    /// A user prompt: run a full turn, streaming events back.
    Prompt(UserInput),
    /// A control operation plus the responder the TUI awaits.
    Control(
        ControlRequest,
        oneshot::Sender<Result<ControlResponse, TransportError>>,
    ),
}

/// A [`TuiTransport`] backed by a remote agent over JSON-RPC.
///
/// A background bridge task owns the [`AgentClient`]: each [`UserInput`]
/// becomes a `prompt` request, every [`AgentEvent`] the server streams
/// during the turn is forwarded to the TUI event loop as it arrives, and
/// [`ControlRequest`]s map onto the protocol's control-plane methods.
///
/// The bridge task exits when the transport is dropped (the input channel
/// closes) or when the connection to the server is lost, after which
/// [`recv`](TuiTransport::recv) yields `None`.
pub struct RemoteTransport {
    /// Send side: the TUI event loop pushes prompts and control ops here.
    input_tx: mpsc::Sender<BridgeMsg>,
    /// Receive side: the TUI event loop reads agent events here.
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,
    /// Out-of-band sender for `cancel` — usable while a turn is streaming.
    cancel: PeerSender,
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
        let (input_tx, mut input_rx) = mpsc::channel::<BridgeMsg>(64);
        let (event_tx, event_rx) = mpsc::unbounded_channel::<AgentEvent>();
        let cancel = client.sender();

        tokio::spawn(async move {
            while let Some(msg) = input_rx.recv().await {
                match msg {
                    BridgeMsg::Prompt(input) => {
                        let forward = event_tx.clone();
                        let result = client
                            .prompt_text_with(input.text, |event| {
                                let _ = forward.send(event);
                            })
                            .await;

                        if let Err(e) = result {
                            tracing::error!("RemoteTransport: turn failed: {e}");
                            // Surface an end-of-turn so the UI leaves its
                            // streaming state, mirroring InProcessTransport's
                            // error behavior.
                            let _ = event_tx.send(AgentEvent::AgentEnd {
                                messages: std::sync::Arc::new(Vec::new()),
                            });
                            if e.code == RpcError::DISCONNECTED {
                                return;
                            }
                        }
                    }
                    BridgeMsg::Control(request, respond) => {
                        let result = handle_control(&client, request).await;
                        let lost = matches!(&result, Err(TransportError::ChannelClosed));
                        let _ = respond.send(result);
                        if lost {
                            return;
                        }
                    }
                }
            }
        });

        Self {
            input_tx,
            event_rx,
            cancel,
        }
    }
}

/// Map one [`ControlRequest`] onto the protocol's control-plane methods.
async fn handle_control(
    client: &AgentClient,
    request: ControlRequest,
) -> Result<ControlResponse, TransportError> {
    match request {
        // Abort is normally sent out-of-band (see `TuiTransport::control`),
        // but handle a queued one too: between turns `cancel` is a no-op on
        // the server, which is the correct meaning here.
        ControlRequest::Abort => {
            client.cancel().await.map_err(|e| map_rpc_error(&e))?;
            Ok(ControlResponse::Ack)
        }
        ControlRequest::ListModels => {
            let listed = client.list_models().await.map_err(|e| map_rpc_error(&e))?;
            Ok(ControlResponse::Models {
                available: listed.available,
                current: listed.current,
            })
        }
        ControlRequest::SetModel(model) => {
            client
                .set_model(model)
                .await
                .map_err(|e| map_rpc_error(&e))?;
            Ok(ControlResponse::Ack)
        }
        ControlRequest::SetThinkingLevel(level) => {
            client
                .set_thinking_level(level)
                .await
                .map_err(|e| map_rpc_error(&e))?;
            Ok(ControlResponse::Ack)
        }
        ControlRequest::SetApprovalMode(mode) => {
            client
                .set_approval_mode(mode)
                .await
                .map_err(|e| map_rpc_error(&e))?;
            Ok(ControlResponse::Ack)
        }
        ControlRequest::QueryApprovalMode => {
            let mode = client
                .approval_mode()
                .await
                .map_err(|e| map_rpc_error(&e))?;
            Ok(ControlResponse::ApprovalMode(mode))
        }
        ControlRequest::SetSystemPrompt(prompt) => {
            client
                .set_system_prompt(prompt)
                .await
                .map_err(|e| map_rpc_error(&e))?;
            Ok(ControlResponse::Ack)
        }
        ControlRequest::Reset => {
            client.reset().await.map_err(|e| map_rpc_error(&e))?;
            Ok(ControlResponse::Ack)
        }
        ControlRequest::EnterPlanMode => {
            client
                .enter_plan_mode()
                .await
                .map_err(|e| map_rpc_error(&e))?;
            Ok(ControlResponse::Ack)
        }
        ControlRequest::ExitPlanMode => {
            client
                .exit_plan_mode()
                .await
                .map_err(|e| map_rpc_error(&e))?;
            Ok(ControlResponse::Ack)
        }
        ControlRequest::SnapshotSession => {
            let snapshot = client
                .session_snapshot()
                .await
                .map_err(|e| map_rpc_error(&e))?;
            Ok(ControlResponse::SessionSnapshot {
                messages: decode_snapshot_messages(snapshot.messages),
                state: snapshot.state,
            })
        }
        ControlRequest::RestoreSession { messages, state } => {
            client
                .session_restore(SessionSnapshot::new(
                    encode_snapshot_messages(&messages),
                    state,
                ))
                .await
                .map_err(|e| map_rpc_error(&e))?;
            Ok(ControlResponse::Ack)
        }
        // `ControlRequest` is `#[non_exhaustive]`: variants added by a newer
        // `swink-agent-tui` are not supported until this crate learns them.
        _ => Err(TransportError::unsupported()),
    }
}

/// Translate an [`RpcError`] into the transport-level error the TUI shows.
///
/// `METHOD_NOT_FOUND` means the server predates the control-plane protocol —
/// the TUI treats that as [`TransportError::Unsupported`] (silent for
/// auto-save, a status note elsewhere). Connection loss maps to
/// [`TransportError::ChannelClosed`] so the bridge task can exit; everything
/// else (including `BUSY` while a turn is in flight) surfaces as an I/O
/// error with the server's message.
fn map_rpc_error(error: &RpcError) -> TransportError {
    match error.code {
        RpcError::METHOD_NOT_FOUND => TransportError::unsupported(),
        RpcError::DISCONNECTED => TransportError::ChannelClosed,
        _ => TransportError::Io(std::io::Error::other(error.to_string())),
    }
}

/// Encode a transcript for `session.restore` in the wire representation:
/// LLM messages as raw [`LlmMessage`] JSON, custom messages as their
/// envelope with a `"_custom": true` marker. Non-serializable messages are
/// skipped with a warning, matching the memory codec.
fn encode_snapshot_messages(messages: &[AgentMessage]) -> Vec<serde_json::Value> {
    let mut encoded = Vec::with_capacity(messages.len());
    for message in messages {
        match message {
            AgentMessage::Llm(llm) => match serde_json::to_value(llm) {
                Ok(value) => encoded.push(value),
                Err(error) => {
                    tracing::warn!("session restore: skipping unserializable message: {error}");
                }
            },
            AgentMessage::Custom(custom) => {
                let Some(mut envelope) = serialize_custom_message(custom.as_ref()) else {
                    tracing::warn!(
                        type_name = custom.type_name().unwrap_or("<unknown>"),
                        "session restore: skipping non-serializable CustomMessage"
                    );
                    continue;
                };
                if let Some(object) = envelope.as_object_mut() {
                    object.insert("_custom".to_string(), serde_json::Value::Bool(true));
                    encoded.push(envelope);
                }
            }
            _ => {
                tracing::warn!("session restore: skipping unrecognized AgentMessage variant");
            }
        }
    }
    encoded
}

/// Decode `session.snapshot` wire messages into [`AgentMessage`]s.
///
/// Custom-message envelopes (`"_custom": true`) are skipped with a warning —
/// no registry exists client-side — and malformed values are skipped the
/// same way, mirroring the memory codec's registry-less load behavior.
fn decode_snapshot_messages(values: Vec<serde_json::Value>) -> Vec<AgentMessage> {
    let mut decoded = Vec::with_capacity(values.len());
    for value in values {
        let is_custom = value
            .get("_custom")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if is_custom {
            tracing::warn!("session snapshot: skipping custom message (no registry client-side)");
            continue;
        }
        match serde_json::from_value::<LlmMessage>(value) {
            Ok(llm) => decoded.push(AgentMessage::Llm(llm)),
            Err(error) => {
                tracing::warn!("session snapshot: skipping undecodable message: {error}");
            }
        }
    }
    decoded
}

impl TuiTransport for RemoteTransport {
    fn send(
        &self,
        input: UserInput,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), TransportError>> + Send + '_>> {
        let tx = self.input_tx.clone();
        Box::pin(async move {
            tx.send(BridgeMsg::Prompt(input))
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

    fn control(
        &mut self,
        request: ControlRequest,
    ) -> Pin<
        Box<dyn std::future::Future<Output = Result<ControlResponse, TransportError>> + Send + '_>,
    > {
        // Abort must not queue behind the in-flight turn it is trying to
        // stop: send the `cancel` notification out-of-band.
        if matches!(request, ControlRequest::Abort) {
            let cancel = self.cancel.clone();
            return Box::pin(async move {
                cancel
                    .notify(
                        swink_agent_rpc::dto::method::CANCEL,
                        &serde_json::Value::Null,
                    )
                    .await
                    .map_err(|_| TransportError::ChannelClosed)?;
                Ok(ControlResponse::Ack)
            });
        }

        let tx = self.input_tx.clone();
        Box::pin(async move {
            let (respond, response) = oneshot::channel();
            tx.send(BridgeMsg::Control(request, respond))
                .await
                .map_err(|_| TransportError::channel_closed())?;
            response
                .await
                .map_err(|_| TransportError::channel_closed())?
        })
    }
}
