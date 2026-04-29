//! JSON-RPC agent client — connects to an `AgentServer` over a Unix socket.

use std::path::Path;

use swink_agent::{AgentEvent, ToolApproval, ToolApprovalRequest};
use tracing::warn;

use crate::dto::{PromptParams, PromptResult, ToolApprovalDto, method};
use crate::jsonrpc::{IncomingMessage, JsonRpcPeer, RpcError};

/// A client that drives a remote `AgentServer` over a Unix socket.
///
/// Connect with [`AgentClient::connect`], then use [`prompt_text`](Self::prompt_text)
/// to interact with the remote agent.
pub struct AgentClient {
    peer: JsonRpcPeer,
    approval_handler: Option<Box<dyn Fn(ToolApprovalRequest) -> ToolApproval + Send + Sync>>,
}

impl AgentClient {
    /// Connect to a running `AgentServer` at the given Unix socket path and
    /// complete the protocol handshake.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket cannot be connected or the handshake fails.
    #[cfg(unix)]
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, RpcError> {
        use crate::dto::{InitializeParams, PROTOCOL_VERSION};

        use tokio::net::UnixStream;

        let stream = UnixStream::connect(path.as_ref())
            .await
            .map_err(|e| RpcError::unavailable(e.to_string()))?;

        let (read, write) = stream.into_split();
        let mut peer = JsonRpcPeer::new(read, write);

        // Send `initialize`.
        peer.sender()
            .notify(
                method::INITIALIZE,
                &InitializeParams {
                    protocol_version: PROTOCOL_VERSION.into(),
                    client: crate::dto::ClientInfo {
                        name: env!("CARGO_PKG_NAME").into(),
                        version: env!("CARGO_PKG_VERSION").into(),
                    },
                },
            )
            .await?;

        // Await `initialized`.
        match peer.recv_incoming().await {
            Some(IncomingMessage::Notification { method: m, params })
                if m == method::INITIALIZED =>
            {
                crate::dto::parse_initialized_params(params)?;
                tracing::debug!("handshake complete");
            }
            Some(other) => {
                warn!("unexpected message during handshake: {other:?}");
                return Err(RpcError::invalid_request(
                    "expected 'initialized' from server",
                ));
            }
            None => return Err(RpcError::disconnected()),
        }

        Ok(Self {
            peer,
            approval_handler: None,
        })
    }

    /// Not available on this platform.
    #[cfg(not(unix))]
    pub async fn connect(_path: impl AsRef<Path>) -> Result<Self, RpcError> {
        std::future::ready(()).await;
        Err(RpcError::unavailable(
            "Unix socket transport requires a Unix host",
        ))
    }

    /// Set a synchronous handler called whenever the server requests tool approval.
    ///
    /// If no handler is set, all tool calls are auto-approved.
    #[must_use]
    pub fn with_approval_handler(
        mut self,
        handler: impl Fn(ToolApprovalRequest) -> ToolApproval + Send + Sync + 'static,
    ) -> Self {
        self.approval_handler = Some(Box::new(handler));
        self
    }

    /// Send a prompt and collect all events, returning when the turn ends.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost or the server returns an error.
    pub async fn prompt_text(
        &mut self,
        text: impl Into<String>,
    ) -> Result<Vec<AgentEvent>, RpcError> {
        let events = self.run_turn(text.into()).await?;
        Ok(events)
    }

    async fn run_turn(&mut self, text: String) -> Result<Vec<AgentEvent>, RpcError> {
        let params = PromptParams {
            text,
            session_id: None,
        };
        let sender = self.peer.sender();

        // Start the prompt request in a background task so we can simultaneously
        // receive the streaming events.
        let prompt_fut = sender.request::<_, PromptResult>(method::PROMPT, &params);
        let mut prompt_fut = std::pin::pin!(prompt_fut);

        let mut events = Vec::new();

        loop {
            tokio::select! {
                result = &mut prompt_fut => {
                    // Prompt request finished; drain any remaining incoming messages.
                    result?;
                    break;
                }
                incoming = self.peer.recv_incoming() => {
                    match incoming {
                        None => return Err(RpcError::disconnected()),
                        Some(IncomingMessage::Notification { method: m, params })
                            if m == method::AGENT_EVENT =>
                        {
                            if let Some(event) = params
                                .and_then(|v| serde_json::from_value::<AgentEvent>(v).ok())
                            {
                                events.push(event);
                            }
                        }
                        Some(IncomingMessage::Request { id, method: m, params })
                            if m == method::TOOL_APPROVE =>
                        {
                            let approval = self.handle_approval(params);
                            let dto = ToolApprovalDto::from(&approval);
                            self.peer.sender().respond_ok(id, dto)?;
                        }
                        Some(IncomingMessage::Request { id, method: m, .. }) => {
                            self.peer.sender().respond_err(
                                id,
                                RpcError::method_not_found(&m),
                            )?;
                        }
                        Some(_) => {}
                    }
                }
            }
        }

        Ok(events)
    }

    fn handle_approval(&self, params: Option<serde_json::Value>) -> ToolApproval {
        let Some(handler) = &self.approval_handler else {
            return ToolApproval::Approved;
        };
        let Some(dto) = params
            .and_then(|v| serde_json::from_value::<crate::dto::ToolApprovalRequestDto>(v).ok())
        else {
            warn!("could not parse tool.approve params; auto-approving");
            return ToolApproval::Approved;
        };
        let req = ToolApprovalRequest {
            tool_call_id: dto.id,
            tool_name: dto.name,
            arguments: dto.arguments,
            requires_approval: dto.requires_approval,
            context: dto.context,
        };
        handler(req)
    }

    /// Send a cancel notification to abort the current turn.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost.
    pub async fn cancel(&self) -> Result<(), RpcError> {
        self.peer
            .sender()
            .notify(method::CANCEL, &serde_json::Value::Null)
            .await
    }

    /// Send a shutdown notification and close the connection.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost before the notification is sent.
    pub async fn shutdown(self) -> Result<(), RpcError> {
        self.peer
            .sender()
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
    }
}
