//! Agent JSON-RPC server — hosts an [`Agent`] behind a Unix socket.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use swink_agent::AgentOptions;

/// A JSON-RPC agent server listening on a Unix socket.
///
/// Use [`AgentServer::bind`] to start listening.  The server accepts one
/// connection at a time; a second concurrent connection is rejected with a
/// `session in use` error.
pub struct AgentServer {
    path: PathBuf,
    factory: Arc<dyn Fn() -> AgentOptions + Send + Sync>,
}

impl AgentServer {
    /// Bind to `path` and serve agents created by `factory`.
    ///
    /// Returns an error if the socket already exists. Use
    /// [`bind_force`](Self::bind_force) to remove it first.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the path already exists or binding fails.
    pub fn bind(
        path: impl AsRef<Path>,
        factory: impl Fn() -> AgentOptions + Send + Sync + 'static,
    ) -> std::io::Result<Self> {
        let path = path.as_ref().to_owned();
        if path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "socket path already exists: {}; remove it or pass --force",
                    path.display()
                ),
            ));
        }
        Ok(Self {
            path,
            factory: Arc::new(factory),
        })
    }

    /// Bind to `path`, removing any existing socket file first.
    ///
    /// # Errors
    ///
    /// Returns `Err` if binding fails.
    pub fn bind_force(
        path: impl AsRef<Path>,
        factory: impl Fn() -> AgentOptions + Send + Sync + 'static,
    ) -> Self {
        let path = path.as_ref().to_owned();
        let _ = std::fs::remove_file(&path);
        Self {
            path,
            factory: Arc::new(factory),
        }
    }

    /// Start the accept loop, running until Ctrl-C.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the Unix listener cannot be bound.
    #[cfg(unix)]
    pub async fn serve(self) -> std::io::Result<()> {
        use tokio::net::UnixListener;
        use tokio::sync::Notify;
        use tracing::{error, info};

        let listener = UnixListener::bind(&self.path)?;

        // Only the owning user may connect.
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;

        info!("swink-agentd listening on {}", self.path.display());
        let _cleanup = SocketCleanup(self.path.clone());

        let session_active = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(Notify::new());
        let shutdown2 = Arc::clone(&shutdown);

        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            info!("shutdown signal received");
            shutdown2.notify_waiters();
        });

        loop {
            tokio::select! {
                accept = listener.accept() => {
                    match accept {
                        Ok((stream, _addr)) => {
                            let active = Arc::clone(&session_active);
                            let factory = Arc::clone(&self.factory);
                            tokio::spawn(handle_connection(stream, active, factory));
                        }
                        Err(e) => {
                            error!("accept error: {e}");
                        }
                    }
                }
                () = shutdown.notified() => {
                    info!("server shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Not available on this platform.
    #[cfg(not(unix))]
    pub async fn serve(self) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Unix socket transport requires a Unix host",
        ))
    }
}

// ─── SocketCleanup ────────────────────────────────────────────────────────────

#[cfg(unix)]
struct SocketCleanup(PathBuf);

#[cfg(unix)]
impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

// ─── Connection handler ───────────────────────────────────────────────────────

#[cfg(unix)]
async fn handle_connection(
    stream: tokio::net::UnixStream,
    session_active: Arc<AtomicBool>,
    factory: Arc<dyn Fn() -> AgentOptions + Send + Sync>,
) {
    use tracing::{info, warn};

    // Peer credential check: only allow connections from the same effective user.
    match peer_uid(&stream) {
        Ok(uid) if uid == effective_uid() => {}
        Ok(uid) => {
            warn!(
                "rejecting connection from uid {uid} (expected {})",
                effective_uid()
            );
            return;
        }
        Err(e) => {
            warn!("peer credential check failed: {e}; rejecting");
            return;
        }
    }

    // Single-session enforcement.
    if session_active
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        info!("rejecting connection: session already in use");
        let (read, write) = stream.into_split();
        let peer = crate::jsonrpc::JsonRpcPeer::new(read, write);
        let _ = peer
            .sender()
            .notify("error", &crate::jsonrpc::RpcError::session_in_use())
            .await;
        return;
    }

    info!("client connected");
    let (read, write) = stream.into_split();
    let mut peer = crate::jsonrpc::JsonRpcPeer::new(read, write);

    let result = run_session(&mut peer, &*factory).await;
    session_active.store(false, Ordering::Release);
    info!("session ended: {:?}", result.err());
}

// ─── Session ──────────────────────────────────────────────────────────────────

#[cfg(unix)]
async fn run_session(
    peer: &mut crate::jsonrpc::JsonRpcPeer,
    factory: &(dyn Fn() -> AgentOptions + Send + Sync),
) -> Result<(), crate::jsonrpc::RpcError> {
    use crate::dto::{
        InitializedParams, PROTOCOL_VERSION, ServerInfo, ToolApprovalDto, ToolApprovalRequestDto,
        method,
    };
    use crate::jsonrpc::{IncomingMessage, RpcError};
    use swink_agent::{Agent, ToolApproval};
    use tracing::{debug, info, warn};

    // Handshake: await `initialize` notification.
    match peer.recv_incoming().await {
        Some(IncomingMessage::Notification { method: m, .. }) if m == method::INITIALIZE => {
            debug!("received initialize");
        }
        Some(other) => {
            warn!("expected 'initialize', got: {other:?}");
            return Err(RpcError::invalid_request("expected 'initialize' first"));
        }
        None => return Err(RpcError::disconnected()),
    }

    peer.sender()
        .notify(
            method::INITIALIZED,
            &InitializedParams {
                protocol_version: PROTOCOL_VERSION.into(),
                server: ServerInfo {
                    name: env!("CARGO_PKG_NAME").into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
            },
        )
        .await?;

    // Wire up tool-approval callback before building the Agent.
    let approval_sender = peer.sender();
    let options = factory().with_approve_tool_async(move |req| {
        let sender = approval_sender.clone();
        async move {
            let dto = ToolApprovalRequestDto::from(&req);
            match sender
                .request::<_, ToolApprovalDto>(method::TOOL_APPROVE, &dto)
                .await
            {
                Ok(d) => ToolApproval::from(d),
                Err(e) => {
                    tracing::warn!("tool approval request failed: {e}; rejecting");
                    ToolApproval::Rejected
                }
            }
        }
    });
    let mut agent = Agent::new(options);

    // Main dispatch loop.
    loop {
        match peer.recv_incoming().await {
            None => break,
            Some(IncomingMessage::Notification { method: m, .. }) if m == method::SHUTDOWN => {
                info!("client requested shutdown");
                break;
            }
            Some(IncomingMessage::Notification { method: m, .. }) if m == method::CANCEL => {
                agent.abort();
            }
            Some(IncomingMessage::Request {
                id,
                method: m,
                params,
            }) if m == method::PROMPT => match run_prompt(peer, &mut agent, params).await {
                Ok(turn_id) => {
                    peer.sender()
                        .respond_ok(id, crate::dto::PromptResult { turn_id })?;
                }
                Err(e) => {
                    peer.sender().respond_err(id, e)?;
                }
            },
            Some(IncomingMessage::Request { id, method: m, .. }) => {
                peer.sender()
                    .respond_err(id, RpcError::method_not_found(&m))?;
            }
            Some(IncomingMessage::Notification { method: m, .. }) => {
                debug!("ignoring unknown notification: {m}");
            }
        }
    }

    Ok(())
}

#[cfg(unix)]
async fn run_prompt(
    peer: &mut crate::jsonrpc::JsonRpcPeer,
    agent: &mut swink_agent::Agent,
    params: Option<serde_json::Value>,
) -> Result<String, crate::jsonrpc::RpcError> {
    use crate::dto::method;
    use crate::jsonrpc::{IncomingMessage, RpcError};
    use futures::StreamExt as _;
    use swink_agent::{AgentMessage, ContentBlock, LlmMessage, UserMessage, now_timestamp};

    let params: crate::dto::PromptParams = params
        .and_then(|v| serde_json::from_value(v).ok())
        .ok_or_else(|| RpcError::invalid_request("missing or invalid prompt params"))?;

    static TURN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let turn_id = TURN_COUNTER
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .to_string();

    let user_msg = AgentMessage::Llm(LlmMessage::User(UserMessage {
        content: vec![ContentBlock::Text { text: params.text }],
        timestamp: now_timestamp(),
        cache_hint: None,
    }));

    let stream = agent
        .prompt_stream(vec![user_msg])
        .map_err(|e| RpcError::internal(e.to_string()))?;
    let mut stream = std::pin::pin!(stream);
    let sender = peer.sender();

    loop {
        tokio::select! {
            event = stream.next() => {
                match event {
                    Some(ev) => sender.notify(method::AGENT_EVENT, &ev).await?,
                    None => break,
                }
            }
            incoming = peer.recv_incoming() => {
                match incoming {
                    None => return Err(RpcError::disconnected()),
                    Some(IncomingMessage::Notification { method: m, .. })
                        if m == method::CANCEL =>
                    {
                        agent.abort();
                    }
                    Some(IncomingMessage::Notification { method: m, .. })
                        if m == method::SHUTDOWN =>
                    {
                        agent.abort();
                        return Err(RpcError::disconnected());
                    }
                    Some(IncomingMessage::Request { id, method: m, .. }) => {
                        peer.sender().respond_err(id, RpcError::method_not_found(&m))?;
                    }
                    Some(_) => {}
                }
            }
        }
    }

    Ok(turn_id)
}

// ─── Peer credential helpers (unix-only) ─────────────────────────────────────

#[cfg(unix)]
fn effective_uid() -> u32 {
    nix::unistd::geteuid().as_raw()
}

#[cfg(all(unix, target_os = "linux"))]
fn peer_uid(stream: &tokio::net::UnixStream) -> std::io::Result<u32> {
    // getsockopt<F: AsFd, O>(fd: &F, opt: O) — UnixStream: AsFd.
    let cred = nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(cred.uid())
}

#[cfg(all(unix, target_os = "macos"))]
fn peer_uid(stream: &tokio::net::UnixStream) -> std::io::Result<u32> {
    // getpeereid<F: AsFd>(fd: F) — UnixStream: AsFd.
    let (uid, _gid) =
        nix::unistd::getpeereid(stream).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(uid.as_raw())
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn peer_uid(_stream: &tokio::net::UnixStream) -> std::io::Result<u32> {
    tracing::warn!("peer credential check not supported on this Unix variant; allowing connection");
    Ok(effective_uid())
}
