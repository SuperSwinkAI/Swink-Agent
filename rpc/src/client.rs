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
                    result?;
                    self.drain_ready_incoming(&mut events)?;
                    break;
                }
                incoming = self.peer.recv_incoming() => {
                    match incoming {
                        None => return Err(RpcError::disconnected()),
                        Some(incoming) => self.handle_incoming(incoming, &mut events)?,
                    }
                }
            }
        }

        Ok(events)
    }

    fn drain_ready_incoming(&mut self, events: &mut Vec<AgentEvent>) -> Result<(), RpcError> {
        while let Some(incoming) = self.peer.try_recv_incoming() {
            self.handle_incoming(incoming, events)?;
        }
        Ok(())
    }

    fn handle_incoming(
        &self,
        incoming: IncomingMessage,
        events: &mut Vec<AgentEvent>,
    ) -> Result<(), RpcError> {
        match incoming {
            IncomingMessage::Notification { method: m, params } if m == method::AGENT_EVENT => {
                if let Some(event) =
                    params.and_then(|v| serde_json::from_value::<AgentEvent>(v).ok())
                {
                    events.push(event);
                }
            }
            IncomingMessage::Request {
                id,
                method: m,
                params,
            } if m == method::TOOL_APPROVE => {
                let approval = self.handle_approval(params);
                let dto = ToolApprovalDto::from(&approval);
                self.peer.sender().respond_ok(id, dto)?;
            }
            IncomingMessage::Request { id, method: m, .. } => {
                self.peer
                    .sender()
                    .respond_err(id, RpcError::method_not_found(&m))?;
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
