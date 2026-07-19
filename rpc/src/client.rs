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
mod tests {
    use swink_agent::{AgentEvent, ToolApproval};
    use tokio::{io::duplex, sync::oneshot};

    use super::*;
    use crate::dto::{PromptResult, ToolApprovalRequestDto};
    use crate::jsonrpc::IncomingMessage;

    fn make_client_pair() -> (AgentClient, JsonRpcPeer) {
        let (client_read, server_write) = duplex(8192);
        let (server_read, client_write) = duplex(8192);
        let client = AgentClient {
            peer: JsonRpcPeer::new(client_read, client_write),
            approval_handler: None,
        };
        let server = JsonRpcPeer::new(server_read, server_write);
        (client, server)
    }

    #[tokio::test]
    async fn prompt_text_collects_agent_events_until_prompt_response() {
        let (mut client, mut server) = make_client_pair();
        let server_sender = server.sender();
        let (client_done_tx, client_done_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let incoming = server.recv_incoming().await.unwrap();
            let IncomingMessage::Request { id, method, params } = incoming else {
                panic!("expected prompt request");
            };

            assert_eq!(method, method::PROMPT);
            let params: PromptParams = serde_json::from_value(params.unwrap()).unwrap();
            assert_eq!(params.text, "hello rpc");

            server_sender
                .notify(method::AGENT_EVENT, &AgentEvent::AgentStart)
                .await
                .unwrap();
            server_sender
                .notify(method::AGENT_EVENT, &AgentEvent::TurnStart)
                .await
                .unwrap();
            server_sender
                .respond_ok(
                    id,
                    PromptResult {
                        turn_id: "1".into(),
                    },
                )
                .await
                .unwrap();
            let _ = client_done_rx.await;
        });

        let events = client.prompt_text("hello rpc").await.unwrap();
        let _ = client_done_tx.send(());

        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], AgentEvent::AgentStart));
        assert!(matches!(events[1], AgentEvent::TurnStart));
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn prompt_text_with_streams_events_through_the_callback() {
        let (mut client, mut server) = make_client_pair();
        let server_sender = server.sender();
        let (client_done_tx, client_done_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let incoming = server.recv_incoming().await.unwrap();
            let IncomingMessage::Request { id, method, .. } = incoming else {
                panic!("expected prompt request");
            };
            assert_eq!(method, method::PROMPT);

            server_sender
                .notify(method::AGENT_EVENT, &AgentEvent::AgentStart)
                .await
                .unwrap();
            server_sender
                .notify(method::AGENT_EVENT, &AgentEvent::TurnStart)
                .await
                .unwrap();
            server_sender
                .respond_ok(
                    id,
                    PromptResult {
                        turn_id: "6".into(),
                    },
                )
                .await
                .unwrap();
            let _ = client_done_rx.await;
        });

        let mut seen = Vec::new();
        client
            .prompt_text_with("hello streaming", |event| seen.push(event))
            .await
            .unwrap();
        let _ = client_done_tx.send(());

        assert_eq!(seen.len(), 2);
        assert!(matches!(seen[0], AgentEvent::AgentStart));
        assert!(matches!(seen[1], AgentEvent::TurnStart));
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn prompt_text_answers_tool_approval_requests() {
        let (client, mut server) = make_client_pair();
        let mut client = client.with_approval_handler(|req| {
            assert_eq!(req.tool_call_id, "call-1");
            assert_eq!(req.tool_name, "dangerous_tool");
            ToolApproval::Rejected
        });
        let server_sender = server.sender();
        let (client_done_tx, client_done_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let incoming = server.recv_incoming().await.unwrap();
            let IncomingMessage::Request { id, method, .. } = incoming else {
                panic!("expected prompt request");
            };
            assert_eq!(method, method::PROMPT);

            let approval = server_sender
                .request::<_, ToolApprovalDto>(
                    method::TOOL_APPROVE,
                    &ToolApprovalRequestDto {
                        id: "call-1".into(),
                        name: "dangerous_tool".into(),
                        arguments: serde_json::json!({"path": "/tmp/example"}),
                        requires_approval: true,
                        context: None,
                    },
                )
                .await
                .unwrap();
            assert!(matches!(approval, ToolApprovalDto::Rejected));

            server_sender
                .respond_ok(
                    id,
                    PromptResult {
                        turn_id: "2".into(),
                    },
                )
                .await
                .unwrap();
            let _ = client_done_rx.await;
        });

        let events = client.prompt_text("run tool").await.unwrap();
        let _ = client_done_tx.send(());

        assert!(events.is_empty());
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn prompt_text_auto_approves_tool_approval_requests_without_handler() {
        let (mut client, mut server) = make_client_pair();
        let server_sender = server.sender();
        let (client_done_tx, client_done_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let incoming = server.recv_incoming().await.unwrap();
            let IncomingMessage::Request { id, method, .. } = incoming else {
                panic!("expected prompt request");
            };
            assert_eq!(method, method::PROMPT);

            let approval = server_sender
                .request::<_, ToolApprovalDto>(
                    method::TOOL_APPROVE,
                    &ToolApprovalRequestDto {
                        id: "call-1".into(),
                        name: "dangerous_tool".into(),
                        arguments: serde_json::json!({"path": "/tmp/example"}),
                        requires_approval: true,
                        context: None,
                    },
                )
                .await
                .unwrap();
            assert!(matches!(approval, ToolApprovalDto::Approved));

            server_sender
                .respond_ok(
                    id,
                    PromptResult {
                        turn_id: "3".into(),
                    },
                )
                .await
                .unwrap();
            let _ = client_done_rx.await;
        });

        let events = client.prompt_text("run tool").await.unwrap();
        let _ = client_done_tx.send(());

        assert!(events.is_empty());
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn prompt_text_rejects_malformed_tool_approval_requests_with_handler() {
        let (client, mut server) = make_client_pair();
        let mut client = client.with_approval_handler(|_| {
            panic!("malformed approval requests must not reach the handler");
        });
        let server_sender = server.sender();
        let (client_done_tx, client_done_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let incoming = server.recv_incoming().await.unwrap();
            let IncomingMessage::Request { id, method, .. } = incoming else {
                panic!("expected prompt request");
            };
            assert_eq!(method, method::PROMPT);

            let approval = server_sender
                .request::<_, ToolApprovalDto>(method::TOOL_APPROVE, &serde_json::json!({}))
                .await
                .unwrap();
            assert!(matches!(approval, ToolApprovalDto::Rejected));

            server_sender
                .respond_ok(
                    id,
                    PromptResult {
                        turn_id: "4".into(),
                    },
                )
                .await
                .unwrap();
            let _ = client_done_rx.await;
        });

        let events = client.prompt_text("run tool").await.unwrap();
        let _ = client_done_tx.send(());

        assert!(events.is_empty());
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn prompt_text_replies_method_not_found_to_unknown_requests() {
        let (mut client, mut server) = make_client_pair();
        let server_sender = server.sender();
        let (client_done_tx, client_done_rx) = oneshot::channel();

        let server_task = tokio::spawn(async move {
            let incoming = server.recv_incoming().await.unwrap();
            let IncomingMessage::Request { id, method, .. } = incoming else {
                panic!("expected prompt request");
            };
            assert_eq!(method, method::PROMPT);

            let err = server_sender
                .request::<_, serde_json::Value>("server.unknown", &serde_json::json!({}))
                .await
                .unwrap_err();
            assert_eq!(err.code, RpcError::METHOD_NOT_FOUND);
            assert_eq!(err.message, "method not found: server.unknown");

            server_sender
                .respond_ok(
                    id,
                    PromptResult {
                        turn_id: "5".into(),
                    },
                )
                .await
                .unwrap();
            let _ = client_done_rx.await;
        });

        let events = client
            .prompt_text("run unknown server request")
            .await
            .unwrap();
        let _ = client_done_tx.send(());

        assert!(events.is_empty());
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn cancel_sends_cancel_notification() {
        let (client, mut server) = make_client_pair();

        client.cancel().await.unwrap();

        let Some(IncomingMessage::Notification { method, params: _ }) =
            server.recv_incoming().await
        else {
            panic!("expected cancel notification");
        };
        assert_eq!(method, method::CANCEL);
    }

    #[tokio::test]
    async fn shutdown_sends_shutdown_notification() {
        let (client, mut server) = make_client_pair();

        client.shutdown().await.unwrap();

        let Some(IncomingMessage::Notification { method, params: _ }) =
            server.recv_incoming().await
        else {
            panic!("expected shutdown notification");
        };
        assert_eq!(method, method::SHUTDOWN);
    }

    #[tokio::test]
    // One scripted server conversation exercising every control helper in
    // order; splitting it would duplicate the fake-server dispatch loop.
    #[allow(clippy::too_many_lines)]
    async fn control_helpers_round_trip_typed_requests() {
        let (client, mut server) = make_client_pair();
        let server_sender = server.sender();

        let server_task = tokio::spawn(async move {
            let mut seen = Vec::new();
            while let Some(incoming) = server.recv_incoming().await {
                let IncomingMessage::Request {
                    id,
                    method: m,
                    params,
                } = incoming
                else {
                    panic!("expected only requests, got {incoming:?}");
                };
                seen.push(m.clone());
                match m.as_str() {
                    method::MODEL_LIST => {
                        server_sender
                            .respond_ok(
                                id,
                                ModelListResult::new(
                                    vec![ModelSpec::new("test", "alt-model")],
                                    ModelSpec::new("test", "test-model"),
                                ),
                            )
                            .await
                            .unwrap();
                    }
                    method::MODEL_SET => {
                        let p: ModelSetParams = serde_json::from_value(params.unwrap()).unwrap();
                        assert_eq!(p.model.model_id, "alt-model");
                        server_sender.respond_ok(id, Ack::new()).await.unwrap();
                    }
                    method::THINKING_SET => {
                        let p: ThinkingSetParams = serde_json::from_value(params.unwrap()).unwrap();
                        assert_eq!(p.level, ThinkingLevel::High);
                        server_sender.respond_ok(id, Ack::new()).await.unwrap();
                    }
                    method::APPROVAL_GET => {
                        server_sender
                            .respond_ok(id, ApprovalGetResult::new(ApprovalMode::Bypassed))
                            .await
                            .unwrap();
                    }
                    method::APPROVAL_SET => {
                        let p: ApprovalSetParams = serde_json::from_value(params.unwrap()).unwrap();
                        assert_eq!(p.mode, ApprovalMode::Enabled);
                        server_sender.respond_ok(id, Ack::new()).await.unwrap();
                    }
                    method::SYSTEM_PROMPT_SET => {
                        let p: SystemPromptSetParams =
                            serde_json::from_value(params.unwrap()).unwrap();
                        assert_eq!(p.prompt, "fresh prompt");
                        server_sender.respond_ok(id, Ack::new()).await.unwrap();
                    }
                    method::AGENT_RESET | method::PLAN_ENTER | method::PLAN_EXIT => {
                        server_sender.respond_ok(id, Ack::new()).await.unwrap();
                    }
                    method::SESSION_SNAPSHOT => {
                        server_sender
                            .respond_ok(
                                id,
                                SessionSnapshot::new(
                                    vec![serde_json::json!({"role": "user"})],
                                    Some(serde_json::json!({"data": {}})),
                                ),
                            )
                            .await
                            .unwrap();
                    }
                    method::SESSION_RESTORE => {
                        let p: SessionSnapshot = serde_json::from_value(params.unwrap()).unwrap();
                        assert_eq!(p.messages.len(), 1);
                        server_sender.respond_ok(id, Ack::new()).await.unwrap();
                    }
                    method::CONTEXT_COMPACT => {
                        server_sender
                            .respond_ok(
                                id,
                                crate::dto::CompactResult::new(Some(
                                    swink_agent::CompactionReport::new(3, 9_000, 2_000, true),
                                )),
                            )
                            .await
                            .unwrap();
                    }
                    other => panic!("unexpected control method: {other}"),
                }
            }
            seen
        });

        let listed = client.list_models().await.unwrap();
        assert_eq!(listed.available.len(), 1);
        assert_eq!(listed.current.model_id, "test-model");

        client
            .set_model(ModelSpec::new("test", "alt-model"))
            .await
            .unwrap();
        client
            .set_thinking_level(ThinkingLevel::High)
            .await
            .unwrap();
        assert_eq!(
            client.approval_mode().await.unwrap(),
            ApprovalMode::Bypassed
        );
        client
            .set_approval_mode(ApprovalMode::Enabled)
            .await
            .unwrap();
        client.set_system_prompt("fresh prompt").await.unwrap();
        client.reset().await.unwrap();
        client.enter_plan_mode().await.unwrap();
        client.exit_plan_mode().await.unwrap();

        let snapshot = client.session_snapshot().await.unwrap();
        assert_eq!(snapshot.messages.len(), 1);
        assert!(snapshot.state.is_some());
        client.session_restore(snapshot).await.unwrap();

        let compacted = client.compact().await.unwrap();
        let report = compacted.report.expect("mock server returns a report");
        assert_eq!(report.dropped_count, 3);

        drop(client);
        let seen = server_task.await.unwrap();
        assert_eq!(
            seen,
            vec![
                method::MODEL_LIST,
                method::MODEL_SET,
                method::THINKING_SET,
                method::APPROVAL_GET,
                method::APPROVAL_SET,
                method::SYSTEM_PROMPT_SET,
                method::AGENT_RESET,
                method::PLAN_ENTER,
                method::PLAN_EXIT,
                method::SESSION_SNAPSHOT,
                method::SESSION_RESTORE,
                method::CONTEXT_COMPACT,
            ]
        );
    }

    #[tokio::test]
    async fn sender_exposes_peer_sender_for_out_of_band_control() {
        let (client, mut server) = make_client_pair();

        // A cloned sender can issue notifications (e.g. `cancel`) from
        // another task while the client itself is busy driving a turn.
        let sender = client.sender();
        sender
            .notify(method::CANCEL, &serde_json::Value::Null)
            .await
            .unwrap();

        let Some(IncomingMessage::Notification { method: m, .. }) = server.recv_incoming().await
        else {
            panic!("expected cancel notification");
        };
        assert_eq!(m, method::CANCEL);
    }

    #[cfg(not(unix))]
    #[tokio::test]
    async fn connect_reports_unix_transport_unavailable_on_non_unix_hosts() {
        let Err(err) = AgentClient::connect("unused.sock").await else {
            panic!("non-Unix client connect should fail");
        };

        assert_eq!(err.code, RpcError::UNAVAILABLE);
        assert!(
            err.message.contains("Unix socket transport"),
            "unexpected error message: {}",
            err.message
        );
    }
}
