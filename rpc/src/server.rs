//! Agent JSON-RPC server — hosts an `Agent` behind a Unix socket.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use swink_agent::AgentOptions;

/// A JSON-RPC agent server listening on a Unix socket.
///
/// Use [`AgentServer::bind`] to start listening.  The server accepts one
/// connection at a time; a second concurrent connection is rejected with a
/// `session in use` error.
pub struct AgentServer {
    path: PathBuf,
    factory: Arc<dyn Fn() -> Result<AgentOptions, String> + Send + Sync>,
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
        factory: impl Fn() -> Result<AgentOptions, String> + Send + Sync + 'static,
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
        factory: impl Fn() -> Result<AgentOptions, String> + Send + Sync + 'static,
    ) -> Self {
        let path = path.as_ref().to_owned();
        let _ = std::fs::remove_file(&path);
        Self {
            path,
            factory: Arc::new(factory),
        }
    }

    /// Start the accept loop, running until Ctrl-C or SIGTERM.
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

        let session_active = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shutdown = Arc::new(Notify::new());
        let shutdown2 = Arc::clone(&shutdown);

        // Install the SIGTERM handler here, before spawning, so a failure to
        // install it propagates as an error from `serve()` immediately rather
        // than silently failing inside a detached task.
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate())?;

        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = sigterm.recv() => {}
            }
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
        let Self { path, factory } = self;
        drop((path, factory));
        std::future::ready(()).await;
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
    session_active: Arc<std::sync::atomic::AtomicBool>,
    factory: Arc<dyn Fn() -> Result<AgentOptions, String> + Send + Sync>,
) {
    use std::sync::atomic::Ordering;

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

#[cfg(any(unix, test))]
// One linear protocol flow (handshake → agent construction → dispatch loop);
// splitting it would scatter the session's state across helpers.
#[allow(clippy::too_many_lines)]
async fn run_session(
    peer: &mut crate::jsonrpc::JsonRpcPeer,
    factory: &(dyn Fn() -> Result<AgentOptions, String> + Send + Sync),
) -> Result<(), crate::jsonrpc::RpcError> {
    use crate::dto::{
        InitializedParams, PROTOCOL_VERSION, ServerInfo, ToolApprovalDto, ToolApprovalRequestDto,
        method, parse_initialize_params,
    };
    use crate::jsonrpc::{IncomingMessage, RpcError};
    use swink_agent::{Agent, ToolApproval};
    use tracing::{debug, info, warn};

    // Handshake: await `initialize` notification.
    match peer.recv_incoming().await {
        Some(IncomingMessage::Notification { method: m, params }) if m == method::INITIALIZE => {
            parse_initialize_params(params)?;
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
    let options = match factory() {
        Ok(options) => options,
        Err(reason) => {
            warn!("failed to build agent options: {reason}");
            let _ = peer
                .sender()
                .notify("error", &RpcError::internal(reason.clone()))
                .await;
            return Err(RpcError::internal(reason));
        }
    };
    let options = options.with_approve_tool_async(move |req| {
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

    // Saved (tools, system prompt) while the agent is in plan mode. The
    // values returned by `Agent::enter_plan_mode` are not serializable, so
    // the server holds them here for the lifetime of the session and feeds
    // them back to `Agent::exit_plan_mode` on `plan.exit`.
    let mut plan_state: PlanModeState = None;

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
                        .respond_ok(id, crate::dto::PromptResult { turn_id })
                        .await?;
                }
                Err(e) => {
                    let end_session = e.code == RpcError::DISCONNECTED;
                    peer.sender().respond_err(id, e).await?;
                    if end_session {
                        break;
                    }
                }
            },
            Some(IncomingMessage::Request {
                id,
                method: m,
                params,
            }) if method::is_control(&m) => {
                match dispatch_control(&mut agent, &mut plan_state, &m, params).await {
                    Ok(result) => peer.sender().respond_ok(id, result).await?,
                    Err(e) => peer.sender().respond_err(id, e).await?,
                }
            }
            Some(IncomingMessage::Request { id, method: m, .. }) => {
                peer.sender()
                    .respond_err(id, RpcError::method_not_found(&m))
                    .await?;
            }
            Some(IncomingMessage::Notification { method: m, .. }) => {
                debug!("ignoring unknown notification: {m}");
            }
        }
    }

    Ok(())
}

#[cfg(any(unix, test))]
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

    let user_msg = AgentMessage::Llm(LlmMessage::User(
        UserMessage::new(vec![ContentBlock::Text { text: params.text }])
            .with_timestamp(now_timestamp()),
    ));

    let stream = agent
        .prompt_stream(vec![user_msg])
        .map_err(|e| RpcError::internal(e.to_string()))?;
    let mut stream = std::pin::pin!(stream);
    let sender = peer.sender();

    loop {
        tokio::select! {
            event = stream.next() => {
                match event {
                    Some(ev) => {
                        // Mirror the event into agent state (the same contract
                        // the TUI follows): without this, `state().messages`
                        // never absorbs the turn, so later turns lose context
                        // and `session.snapshot` reads a stale transcript.
                        agent.handle_stream_event(&ev);
                        sender.notify(method::AGENT_EVENT, &ev).await?;
                    }
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
                    // Control-plane requests are rejected (not dropped, and
                    // not method_not_found) while a turn is in flight — the
                    // `cancel` notification above is the mid-turn-safe way
                    // to regain control.
                    Some(IncomingMessage::Request { id, method: m, .. })
                        if method::is_control(&m) =>
                    {
                        peer.sender().respond_err(id, RpcError::busy()).await?;
                    }
                    Some(IncomingMessage::Request { id, method: m, .. }) => {
                        peer.sender()
                            .respond_err(id, RpcError::method_not_found(&m))
                            .await?;
                    }
                    Some(_) => {}
                }
            }
        }
    }

    Ok(turn_id)
}

// ─── Control plane ────────────────────────────────────────────────────────────

/// Saved (tools, system prompt) held by the session while plan mode is active.
///
/// `Some` means the agent is currently in plan mode.
#[cfg(any(unix, test))]
type PlanModeState = Option<(Vec<Arc<dyn swink_agent::AgentTool>>, String)>;

/// Handle one control-plane request (protocol 1.1) between turns.
///
/// Returns the JSON result to send back, or the [`RpcError`](crate::jsonrpc::RpcError)
/// to respond with. Only called from the main dispatch loop in `run_session`;
/// while a turn is in flight `run_prompt` answers control requests with
/// [`RpcError::busy`](crate::jsonrpc::RpcError::busy) instead.
#[cfg(any(unix, test))]
async fn dispatch_control(
    agent: &mut swink_agent::Agent,
    plan_state: &mut PlanModeState,
    method_name: &str,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, crate::jsonrpc::RpcError> {
    use crate::dto::{
        Ack, ApprovalGetResult, ApprovalSetParams, CompactResult, ModelListResult, ModelSetParams,
        SystemPromptSetParams, ThinkingSetParams, method,
    };
    use crate::jsonrpc::RpcError;

    fn encode<T: serde::Serialize>(value: T) -> Result<serde_json::Value, RpcError> {
        serde_json::to_value(value).map_err(|e| RpcError::internal(e.to_string()))
    }

    match method_name {
        method::MODEL_LIST => {
            let state = agent.state();
            encode(ModelListResult::new(
                state.available_models.clone(),
                state.model.clone(),
            ))
        }
        method::MODEL_SET => {
            let p: ModelSetParams = parse_control_params(params, method::MODEL_SET)?;
            agent.set_model(p.model);
            encode(Ack::new())
        }
        method::THINKING_SET => {
            let p: ThinkingSetParams = parse_control_params(params, method::THINKING_SET)?;
            agent.set_thinking_level(p.level);
            encode(Ack::new())
        }
        method::APPROVAL_GET => encode(ApprovalGetResult::new(agent.approval_mode())),
        method::APPROVAL_SET => {
            let p: ApprovalSetParams = parse_control_params(params, method::APPROVAL_SET)?;
            agent.set_approval_mode(p.mode);
            encode(Ack::new())
        }
        method::SYSTEM_PROMPT_SET => {
            let p: SystemPromptSetParams = parse_control_params(params, method::SYSTEM_PROMPT_SET)?;
            agent.set_system_prompt(p.prompt);
            encode(Ack::new())
        }
        method::AGENT_RESET => {
            agent.reset();
            encode(Ack::new())
        }
        method::PLAN_ENTER => {
            if plan_state.is_some() {
                return Err(RpcError::invalid_request("already in plan mode"));
            }
            *plan_state = Some(agent.enter_plan_mode());
            encode(Ack::new())
        }
        method::PLAN_EXIT => {
            let (saved_tools, saved_prompt) = plan_state
                .take()
                .ok_or_else(|| RpcError::invalid_request("not in plan mode"))?;
            agent.exit_plan_mode(saved_tools, saved_prompt);
            encode(Ack::new())
        }
        method::CONTEXT_COMPACT => {
            // Only dispatched between turns (see `is_control`), so the
            // agent-side `Err(AlreadyRunning)` guard is a backstop.
            let report = agent
                .compact_context()
                .await
                .map_err(|e| RpcError::invalid_request(e.to_string()))?;
            encode(CompactResult::new(report))
        }
        method::SESSION_SNAPSHOT => encode(session_snapshot(agent)?),
        method::SESSION_RESTORE => {
            session_restore(
                agent,
                parse_control_params(params, method::SESSION_RESTORE)?,
            )?;
            encode(Ack::new())
        }
        // Unreachable while callers gate on `method::is_control`, but a new
        // method added to `is_control` without a dispatch arm must fail
        // loudly rather than fall through to a success path.
        other => Err(RpcError::method_not_found(other)),
    }
}

/// Build the `session.snapshot` result from the agent's transcript and
/// session state.
#[cfg(any(unix, test))]
fn session_snapshot(
    agent: &swink_agent::Agent,
) -> Result<crate::dto::SessionSnapshot, crate::jsonrpc::RpcError> {
    use crate::jsonrpc::RpcError;

    let messages = agent
        .state()
        .messages
        .iter()
        .filter_map(snapshot_message)
        .collect();
    let state = {
        let guard = agent
            .session_state()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        serde_json::to_value(&*guard).map_err(|e| RpcError::internal(e.to_string()))?
    };
    Ok(crate::dto::SessionSnapshot::new(messages, Some(state)))
}

/// Apply a `session.restore` snapshot: replace the agent's transcript and
/// session state, mirroring the TUI's session-load write-back.
#[cfg(any(unix, test))]
fn session_restore(
    agent: &mut swink_agent::Agent,
    snapshot: crate::dto::SessionSnapshot,
) -> Result<(), crate::jsonrpc::RpcError> {
    use crate::jsonrpc::RpcError;

    let state = snapshot
        .state
        .map(serde_json::from_value::<swink_agent::SessionState>)
        .transpose()
        .map_err(|e| RpcError::invalid_request(format!("invalid session.restore state: {e}")))?
        .unwrap_or_default();
    let mut restored = Vec::with_capacity(snapshot.messages.len());
    let registry = agent.custom_message_registry();
    for value in snapshot.messages {
        if let Some(message) = restore_message(value, registry)? {
            restored.push(message);
        }
    }
    agent.set_messages(restored);
    *agent
        .session_state()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
    Ok(())
}

/// Parse control-plane request params, mirroring the handshake parsers'
/// error shape (`invalid request` with the failing method named).
#[cfg(any(unix, test))]
fn parse_control_params<T>(
    params: Option<serde_json::Value>,
    method_name: &str,
) -> Result<T, crate::jsonrpc::RpcError>
where
    T: serde::de::DeserializeOwned,
{
    use crate::jsonrpc::RpcError;

    let Some(params) = params else {
        return Err(RpcError::invalid_request(format!(
            "missing {method_name} params"
        )));
    };
    serde_json::from_value(params)
        .map_err(|e| RpcError::invalid_request(format!("invalid {method_name} params: {e}")))
}

/// Encode one [`AgentMessage`](swink_agent::AgentMessage) for `session.snapshot`.
///
/// Mirrors the JSONL representation used by `swink-agent-memory`: LLM
/// messages as raw `LlmMessage` JSON, custom messages as their
/// `serialize_custom_message` envelope with a `"_custom": true` marker.
/// Non-serializable custom messages (and unknown future variants) are
/// skipped with a warning, matching the store's behavior.
#[cfg(any(unix, test))]
fn snapshot_message(msg: &swink_agent::AgentMessage) -> Option<serde_json::Value> {
    use swink_agent::{AgentMessage, serialize_custom_message};

    match msg {
        AgentMessage::Llm(llm) => serde_json::to_value(llm).ok(),
        AgentMessage::Custom(custom) => {
            let Some(mut envelope) = serialize_custom_message(custom.as_ref()) else {
                tracing::warn!(
                    type_name = custom.type_name().unwrap_or("<unknown>"),
                    "session.snapshot: skipping non-serializable CustomMessage"
                );
                return None;
            };
            envelope
                .as_object_mut()
                .expect("custom message envelope must be an object")
                .insert("_custom".to_string(), serde_json::Value::Bool(true));
            Some(envelope)
        }
        // `AgentMessage` is `#[non_exhaustive]`: skip variants this build
        // does not know how to encode, as the memory codec does.
        _ => {
            tracing::warn!("session.snapshot: skipping unrecognized AgentMessage variant");
            None
        }
    }
}

/// Decode one `session.restore` message value back into an
/// [`AgentMessage`](swink_agent::AgentMessage), mirroring the memory crate's
/// JSONL decoding: values marked `"_custom": true` go through the agent's
/// [`CustomMessageRegistry`](swink_agent::CustomMessageRegistry) (and are
/// skipped when the agent has none), everything else must parse as a raw
/// `LlmMessage`.
#[cfg(any(unix, test))]
fn restore_message(
    value: serde_json::Value,
    registry: Option<&swink_agent::CustomMessageRegistry>,
) -> Result<Option<swink_agent::AgentMessage>, crate::jsonrpc::RpcError> {
    use crate::jsonrpc::RpcError;
    use swink_agent::{AgentMessage, LlmMessage, restore_single_custom};

    if value.get("_custom").and_then(serde_json::Value::as_bool) == Some(true) {
        return restore_single_custom(registry, &value)
            .map(|opt| opt.map(AgentMessage::Custom))
            .map_err(|e| {
                RpcError::invalid_request(format!("invalid custom message in session.restore: {e}"))
            });
    }

    serde_json::from_value::<LlmMessage>(value)
        .map(|m| Some(AgentMessage::Llm(m)))
        .map_err(|e| RpcError::invalid_request(format!("invalid message in session.restore: {e}")))
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

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
