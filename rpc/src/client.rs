//! JSON-RPC agent client — connects to an `AgentServer` over a Unix socket.

use std::path::Path;

use swink_agent::{
    AgentEvent, ApprovalMode, ModelSpec, ThinkingLevel, ToolApproval, ToolApprovalRequest,
};
use tracing::warn;

use crate::dto::{
    Ack, ApprovalGetResult, ApprovalSetParams, ModelListResult, ModelSetParams, PromptParams,
    PromptResult, SessionSnapshot, SystemPromptSetParams, ThinkingSetParams, ToolApprovalDto,
    method,
};
use crate::jsonrpc::{IncomingMessage, JsonRpcPeer, PeerSender, RpcError};

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
    /// For live delivery — a UI rendering events as the agent produces them —
    /// use [`prompt_text_with`](Self::prompt_text_with) instead.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost or the server returns an error.
    pub async fn prompt_text(
        &mut self,
        text: impl Into<String>,
    ) -> Result<Vec<AgentEvent>, RpcError> {
        let mut events = Vec::new();
        self.run_turn(text.into(), &mut |event| events.push(event))
            .await?;
        Ok(events)
    }

    /// Send a prompt, invoking `on_event` for each [`AgentEvent`] as it
    /// arrives, and return when the turn ends.
    ///
    /// Unlike [`prompt_text`](Self::prompt_text), events are delivered while
    /// the turn is still running, so a caller can stream them into a UI or a
    /// channel instead of waiting for the batch at turn end.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost or the server returns an error.
    pub async fn prompt_text_with(
        &mut self,
        text: impl Into<String>,
        mut on_event: impl FnMut(AgentEvent) + Send,
    ) -> Result<(), RpcError> {
        self.run_turn(text.into(), &mut on_event).await
    }

    async fn run_turn(
        &mut self,
        text: String,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> Result<(), RpcError> {
        let params = PromptParams {
            text,
            session_id: None,
        };
        let sender = self.peer.sender();

        // Start the prompt request in a background task so we can simultaneously
        // receive the streaming events.
        let prompt_fut = sender.request::<_, PromptResult>(method::PROMPT, &params);
        let mut prompt_fut = std::pin::pin!(prompt_fut);

        loop {
            tokio::select! {
                result = &mut prompt_fut => {
                    result?;
                    self.drain_ready_incoming(on_event).await?;
                    break;
                }
                incoming = self.peer.recv_incoming() => {
                    match incoming {
                        None => return Err(RpcError::disconnected()),
                        Some(incoming) => self.handle_incoming(incoming, on_event).await?,
                    }
                }
            }
        }

        Ok(())
    }

    async fn drain_ready_incoming(
        &mut self,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> Result<(), RpcError> {
        while let Some(incoming) = self.peer.try_recv_incoming() {
            self.handle_incoming(incoming, on_event).await?;
        }
        Ok(())
    }

    async fn handle_incoming(
        &self,
        incoming: IncomingMessage,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
    ) -> Result<(), RpcError> {
        match incoming {
            IncomingMessage::Notification { method: m, params } if m == method::AGENT_EVENT => {
                if let Some(event) =
                    params.and_then(|v| serde_json::from_value::<AgentEvent>(v).ok())
                {
                    on_event(event);
                }
            }
            IncomingMessage::Request {
                id,
                method: m,
                params,
            } if m == method::TOOL_APPROVE => {
                let approval = self.handle_approval(params);
                let dto = ToolApprovalDto::from(&approval);
                self.peer.sender().respond_ok(id, dto).await?;
            }
            IncomingMessage::Request { id, method: m, .. } => {
                self.peer
                    .sender()
                    .respond_err(id, RpcError::method_not_found(&m))
                    .await?;
            }
            IncomingMessage::Notification { .. } => {}
        }
        Ok(())
    }

    fn handle_approval(&self, params: Option<serde_json::Value>) -> ToolApproval {
        let Some(handler) = &self.approval_handler else {
            return ToolApproval::Approved;
        };
        let Some(dto) = params
            .and_then(|v| serde_json::from_value::<crate::dto::ToolApprovalRequestDto>(v).ok())
        else {
            warn!("could not parse tool.approve params; rejecting");
            return ToolApproval::Rejected;
        };
        let req = ToolApprovalRequest::new(dto.id, dto.name, dto.arguments, dto.requires_approval);
        let req = match dto.context {
            Some(context) => req.with_context(context),
            None => req,
        };
        handler(req)
    }

    // ─── Control plane ────────────────────────────────────────────────────

    /// Return a cloneable [`PeerSender`] for this connection, for issuing
    /// requests and notifications from another task while a
    /// [`prompt_text_with`](Self::prompt_text_with) turn is in flight.
    ///
    /// Note that the server rejects control-plane requests (`model.*`,
    /// `approval.*`, `plan.*`, `session.*`, `thinking.set`,
    /// `system_prompt.set`, `agent.reset`) with [`RpcError::BUSY`]
    /// while a turn is running; the
    /// `cancel` notification (see [`cancel`](Self::cancel)) is the only
    /// mid-turn-safe control operation.
    #[must_use]
    pub fn sender(&self) -> PeerSender {
        self.peer.sender()
    }

    /// Send a control-plane request whose result is an empty [`Ack`].
    async fn ack_request<P: serde::Serialize + Sync>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<(), RpcError> {
        let _ack: Ack = self.peer.sender().request(method, params).await?;
        Ok(())
    }

    /// List the models available on the server and the current model.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost, a turn is in progress
    /// ([`RpcError::BUSY`]), or the server returns an error.
    pub async fn list_models(&self) -> Result<ModelListResult, RpcError> {
        self.peer
            .sender()
            .request(method::MODEL_LIST, &serde_json::json!({}))
            .await
    }

    /// Switch the remote agent to `model`.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost, a turn is in progress
    /// ([`RpcError::BUSY`]), or the server returns an error.
    pub async fn set_model(&self, model: ModelSpec) -> Result<(), RpcError> {
        self.ack_request(method::MODEL_SET, &ModelSetParams::new(model))
            .await
    }

    /// Set the thinking level on the remote agent's current model.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost, a turn is in progress
    /// ([`RpcError::BUSY`]), or the server returns an error.
    pub async fn set_thinking_level(&self, level: ThinkingLevel) -> Result<(), RpcError> {
        self.ack_request(method::THINKING_SET, &ThinkingSetParams::new(level))
            .await
    }

    /// Get the remote agent's current tool-approval mode.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost, a turn is in progress
    /// ([`RpcError::BUSY`]), or the server returns an error.
    pub async fn approval_mode(&self) -> Result<ApprovalMode, RpcError> {
        let result: ApprovalGetResult = self
            .peer
            .sender()
            .request(method::APPROVAL_GET, &serde_json::json!({}))
            .await?;
        Ok(result.mode)
    }

    /// Set the remote agent's tool-approval mode.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost, a turn is in progress
    /// ([`RpcError::BUSY`]), or the server returns an error.
    pub async fn set_approval_mode(&self, mode: ApprovalMode) -> Result<(), RpcError> {
        self.ack_request(method::APPROVAL_SET, &ApprovalSetParams::new(mode))
            .await
    }

    /// Replace the remote agent's system prompt.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost, a turn is in progress
    /// ([`RpcError::BUSY`]), or the server returns an error.
    pub async fn set_system_prompt(&self, prompt: impl Into<String>) -> Result<(), RpcError> {
        self.ack_request(
            method::SYSTEM_PROMPT_SET,
            &SystemPromptSetParams::new(prompt),
        )
        .await
    }

    /// Reset the remote agent, clearing its transcript, queues, and error.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost, a turn is in progress
    /// ([`RpcError::BUSY`]), or the server returns an error.
    pub async fn reset(&self) -> Result<(), RpcError> {
        self.ack_request(method::AGENT_RESET, &serde_json::json!({}))
            .await
    }

    /// Run the remote agent's context transformers against its stored
    /// history now (manual compaction, e.g. a `/compact` command).
    ///
    /// Returns the last transformer's report, or `None` when no transformer
    /// is configured or every transformer declined (history under budget).
    ///
    /// # Errors
    ///
    /// Returns [`RpcError`] on transport failure or when the server rejects
    /// the request; servers that predate `context.compact` answer
    /// `METHOD_NOT_FOUND`.
    pub async fn compact(&self) -> Result<crate::dto::CompactResult, RpcError> {
        self.peer
            .sender()
            .request(method::CONTEXT_COMPACT, &serde_json::json!({}))
            .await
    }

    /// Put the remote agent into plan mode (read-only tools, plan-mode
    /// system prompt addendum). The server holds the saved tools and prompt
    /// until [`exit_plan_mode`](Self::exit_plan_mode).
    ///
    /// # Errors
    ///
    /// Returns an error if the agent is already in plan mode
    /// ([`RpcError::INVALID_REQUEST`]), the connection is lost, or a turn is
    /// in progress ([`RpcError::BUSY`]).
    pub async fn enter_plan_mode(&self) -> Result<(), RpcError> {
        self.ack_request(method::PLAN_ENTER, &serde_json::json!({}))
            .await
    }

    /// Take the remote agent out of plan mode, restoring the tools and
    /// system prompt saved by [`enter_plan_mode`](Self::enter_plan_mode).
    ///
    /// # Errors
    ///
    /// Returns an error if the agent is not in plan mode
    /// ([`RpcError::INVALID_REQUEST`]), the connection is lost, or a turn is
    /// in progress ([`RpcError::BUSY`]).
    pub async fn exit_plan_mode(&self) -> Result<(), RpcError> {
        self.ack_request(method::PLAN_EXIT, &serde_json::json!({}))
            .await
    }

    /// Fetch a snapshot of the remote agent's transcript and session state.
    ///
    /// The returned [`SessionSnapshot`] uses the same per-message
    /// representation `swink-agent-memory` writes to JSONL, so it can be fed
    /// to a `SessionStore` (and later passed back to
    /// [`session_restore`](Self::session_restore)).
    ///
    /// # Errors
    ///
    /// Returns an error if the connection is lost, a turn is in progress
    /// ([`RpcError::BUSY`]), or the server returns an error.
    pub async fn session_snapshot(&self) -> Result<SessionSnapshot, RpcError> {
        self.peer
            .sender()
            .request(method::SESSION_SNAPSHOT, &serde_json::json!({}))
            .await
    }

    /// Replace the remote agent's transcript and session state with
    /// `snapshot` (as produced by [`session_snapshot`](Self::session_snapshot)
    /// or loaded from a session store).
    ///
    /// # Errors
    ///
    /// Returns an error if a message in the snapshot cannot be decoded
    /// ([`RpcError::INVALID_REQUEST`]), the connection is lost, or a turn is
    /// in progress ([`RpcError::BUSY`]).
    pub async fn session_restore(&self, snapshot: SessionSnapshot) -> Result<(), RpcError> {
        self.ack_request(method::SESSION_RESTORE, &snapshot).await
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

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
