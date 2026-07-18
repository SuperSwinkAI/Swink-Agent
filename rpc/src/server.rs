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
                match dispatch_control(&mut agent, &mut plan_state, &m, params) {
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
fn dispatch_control(
    agent: &mut swink_agent::Agent,
    plan_state: &mut PlanModeState,
    method_name: &str,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, crate::jsonrpc::RpcError> {
    use crate::dto::{
        Ack, ApprovalGetResult, ApprovalSetParams, ModelListResult, ModelSetParams,
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
mod tests {
    use std::io::ErrorKind;
    use std::sync::Arc;
    use std::time::Duration;

    use swink_agent::{
        AgentEvent, AgentOptions, AgentTool, ApprovalMode, LlmMessage, ModelSpec, StreamFn,
        ThinkingLevel,
    };
    use tokio::io::duplex;

    use super::*;
    use crate::dto::{
        Ack, ApprovalGetResult, ApprovalSetParams, ClientInfo, InitializeParams, ModelListResult,
        ModelSetParams, PROTOCOL_VERSION, PromptParams, PromptResult, SessionSnapshot,
        SystemPromptSetParams, ThinkingSetParams, ToolApprovalDto, ToolApprovalRequestDto, method,
    };
    use crate::jsonrpc::{IncomingMessage, JsonRpcPeer};

    fn make_peer_pair() -> (JsonRpcPeer, JsonRpcPeer) {
        let (client_read, server_write) = duplex(8192);
        let (server_read, client_write) = duplex(8192);
        (
            JsonRpcPeer::new(client_read, client_write),
            JsonRpcPeer::new(server_read, server_write),
        )
    }

    fn test_agent_options(response: &'static str) -> AgentOptions {
        let stream_fn: Arc<dyn StreamFn> = Arc::new(
            swink_agent::testing::SimpleMockStreamFn::from_text(response),
        );
        AgentOptions::new(
            "test system",
            swink_agent::testing::default_model(),
            stream_fn,
            swink_agent::testing::default_convert,
        )
    }

    fn approval_blocking_agent_options() -> AgentOptions {
        let stream_fn: Arc<dyn StreamFn> = Arc::new(swink_agent::testing::MockStreamFn::new(vec![
            swink_agent::testing::tool_call_events("call-1", "blocked_tool", r"{}"),
        ]));
        let tool = Arc::new(
            swink_agent::testing::MockTool::new("blocked_tool").with_requires_approval(true),
        );

        AgentOptions::new(
            "test system",
            swink_agent::testing::default_model(),
            stream_fn,
            swink_agent::testing::default_convert,
        )
        .with_tools(vec![tool as Arc<dyn AgentTool>])
    }

    #[test]
    fn bind_rejects_existing_socket_path_without_force() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("swink.sock");
        std::fs::write(&path, b"stale socket placeholder").unwrap();

        let err = match AgentServer::bind(&path, || Ok(test_agent_options("unused"))) {
            Ok(_) => panic!("bind should reject existing socket path"),
            Err(err) => err,
        };

        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert!(
            err.to_string().contains("remove it or pass --force"),
            "unexpected bind error: {err}"
        );
        assert!(
            path.exists(),
            "bind without force must not remove the existing path"
        );
    }

    #[test]
    fn bind_force_removes_existing_stale_socket_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("swink.sock");
        std::fs::write(&path, b"stale socket placeholder").unwrap();

        let _server = AgentServer::bind_force(&path, || Ok(test_agent_options("unused")));

        assert!(
            !path.exists(),
            "bind_force should remove a stale socket path before serving"
        );
    }

    async fn initialize(peer: &mut JsonRpcPeer) {
        peer.sender()
            .notify(
                method::INITIALIZE,
                &InitializeParams {
                    protocol_version: PROTOCOL_VERSION.into(),
                    client: ClientInfo {
                        name: "test-client".into(),
                        version: "0.1.0".into(),
                    },
                },
            )
            .await
            .unwrap();

        let Some(IncomingMessage::Notification { method: m, .. }) = peer.recv_incoming().await
        else {
            panic!("expected initialized notification");
        };
        assert_eq!(m, method::INITIALIZED);
    }

    #[tokio::test]
    async fn run_session_streams_prompt_events_and_turn_response() {
        let (mut client, mut server) = make_peer_pair();

        let server_task = tokio::spawn(async move {
            run_session(&mut server, &|| {
                Ok(test_agent_options("hello from rpc server"))
            })
            .await
            .unwrap();
        });

        initialize(&mut client).await;

        let sender = client.sender();
        let params = PromptParams {
            text: "hello rpc".into(),
            session_id: None,
        };
        let prompt = sender.request::<_, PromptResult>(method::PROMPT, &params);
        let mut prompt = std::pin::pin!(prompt);
        let mut events = Vec::new();
        let result = loop {
            tokio::select! {
                result = &mut prompt => {
                    let result = result.unwrap();
                    while let Some(incoming) = client.try_recv_incoming() {
                        collect_agent_event(incoming, &mut events);
                    }
                    break result;
                }
                incoming = client.recv_incoming() => {
                    let incoming = incoming.expect("server should stay connected while prompt runs");
                    collect_agent_event(incoming, &mut events);
                }
            }
        };

        assert!(!result.turn_id.is_empty());
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::TurnStart)),
            "server should stream turn lifecycle events"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::MessageEnd { message } if message
                    .content
                    .iter()
                    .any(|block| matches!(block, swink_agent::ContentBlock::Text { text } if text == "hello from rpc server")))),
            "server should stream the assistant response body"
        );

        client
            .sender()
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
            .unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn run_session_rejects_invalid_prompt_params_without_ending_session() {
        let (mut client, mut server) = make_peer_pair();

        let server_task = tokio::spawn(async move {
            run_session(&mut server, &|| Ok(test_agent_options("valid follow-up")))
                .await
                .unwrap();
        });

        initialize(&mut client).await;

        let sender = client.sender();
        let err = sender
            .request::<_, PromptResult>(
                method::PROMPT,
                &serde_json::json!({
                    "session_id": "missing-text"
                }),
            )
            .await
            .unwrap_err();

        assert_eq!(err.code, crate::jsonrpc::RpcError::INVALID_REQUEST);
        assert!(
            err.message.contains("missing or invalid prompt params"),
            "unexpected prompt error: {}",
            err.message
        );

        let params = PromptParams {
            text: "recover after invalid params".into(),
            session_id: None,
        };
        let prompt = sender.request::<_, PromptResult>(method::PROMPT, &params);
        let mut prompt = std::pin::pin!(prompt);
        let mut events = Vec::new();
        let result = loop {
            tokio::select! {
                result = &mut prompt => {
                    let result = result.unwrap();
                    while let Some(incoming) = client.try_recv_incoming() {
                        collect_agent_event(incoming, &mut events);
                    }
                    break result;
                }
                incoming = client.recv_incoming() => {
                    let incoming = incoming.expect("server should stay connected after rejecting invalid prompt");
                    collect_agent_event(incoming, &mut events);
                }
            }
        };

        assert!(!result.turn_id.is_empty());
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::MessageEnd { message } if message
                    .content
                    .iter()
                    .any(|block| matches!(block, swink_agent::ContentBlock::Text { text } if text == "valid follow-up"))
            )),
            "server should continue serving valid prompts after an invalid request"
        );

        client
            .sender()
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
            .unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn run_session_rejects_unknown_requests_without_ending_session() {
        let (mut client, mut server) = make_peer_pair();

        let server_task = tokio::spawn(async move {
            run_session(&mut server, &|| Ok(test_agent_options("unused")))
                .await
                .unwrap();
        });

        initialize(&mut client).await;

        let sender = client.sender();
        let err = sender
            .request::<_, serde_json::Value>("rpc.unknown", &serde_json::json!({}))
            .await
            .unwrap_err();

        assert_eq!(err.code, crate::jsonrpc::RpcError::METHOD_NOT_FOUND);
        assert_eq!(err.message, "method not found: rpc.unknown");

        sender
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
            .unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn run_session_ignores_idle_cancel_without_ending_session() {
        let (mut client, mut server) = make_peer_pair();

        let server_task = tokio::spawn(async move {
            run_session(&mut server, &|| Ok(test_agent_options("after idle cancel")))
                .await
                .unwrap();
        });

        initialize(&mut client).await;

        let sender = client.sender();
        sender
            .notify(method::CANCEL, &serde_json::Value::Null)
            .await
            .unwrap();

        let params = PromptParams {
            text: "still accepts prompts".into(),
            session_id: None,
        };
        let prompt = sender.request::<_, PromptResult>(method::PROMPT, &params);
        let mut prompt = std::pin::pin!(prompt);
        let mut events = Vec::new();
        let result = loop {
            tokio::select! {
                result = &mut prompt => {
                    let result = result.unwrap();
                    while let Some(incoming) = client.try_recv_incoming() {
                        collect_agent_event(incoming, &mut events);
                    }
                    break result;
                }
                incoming = client.recv_incoming() => {
                    let incoming = incoming.expect("server should stay connected after idle cancel");
                    collect_agent_event(incoming, &mut events);
                }
            }
        };

        assert!(!result.turn_id.is_empty());
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::MessageEnd { message } if message
                    .content
                    .iter()
                    .any(|block| matches!(block, swink_agent::ContentBlock::Text { text } if text == "after idle cancel"))
            )),
            "server should keep serving prompts after an idle cancel notification"
        );

        sender
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
            .unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn run_session_shutdown_during_prompt_ends_session() {
        let (mut client, mut server) = make_peer_pair();

        let server_task = tokio::spawn(async move {
            run_session(&mut server, &|| Ok(approval_blocking_agent_options()))
                .await
                .unwrap();
        });

        initialize(&mut client).await;

        let sender = client.sender();
        let params = PromptParams {
            text: "start a long prompt".into(),
            session_id: None,
        };
        let prompt_sender = sender.clone();
        let prompt_task = tokio::spawn(async move {
            prompt_sender
                .request::<_, PromptResult>(method::PROMPT, &params)
                .await
        });
        let mut prompt_task = std::pin::pin!(prompt_task);

        loop {
            tokio::select! {
                result = &mut prompt_task => {
                    panic!("prompt resolved before tool approval request: {result:?}");
                }
                incoming = client.recv_incoming() => {
                    match incoming.expect("server should request approval before shutdown") {
                        IncomingMessage::Request { method: m, .. } if m == method::TOOL_APPROVE => break,
                        IncomingMessage::Notification { method: m, .. } if m == method::AGENT_EVENT => {}
                        other => panic!("unexpected message while awaiting tool approval: {other:?}"),
                    }
                }
            }
        }

        sender
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
            .unwrap();

        let err = prompt_task.await.unwrap().unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::RpcError::DISCONNECTED);

        tokio::time::timeout(Duration::from_secs(1), server_task)
            .await
            .expect("shutdown during a prompt should end the session")
            .unwrap();
    }

    #[tokio::test]
    async fn run_session_round_trips_tool_approval_during_prompt() {
        let (mut client, mut server) = make_peer_pair();
        let stream_fn: Arc<dyn StreamFn> = Arc::new(swink_agent::testing::MockStreamFn::new(vec![
            swink_agent::testing::tool_call_events(
                "call-1",
                "dangerous_tool",
                r#"{"path":"/tmp/example"}"#,
            ),
            swink_agent::testing::text_only_events("done after approval"),
        ]));
        let tool = Arc::new(
            swink_agent::testing::MockTool::new("dangerous_tool").with_requires_approval(true),
        );
        let executed_tool = Arc::clone(&tool);

        let server_task = tokio::spawn(async move {
            let factory = || {
                Ok(AgentOptions::new(
                    "test system",
                    swink_agent::testing::default_model(),
                    Arc::clone(&stream_fn),
                    swink_agent::testing::default_convert,
                )
                .with_tools(vec![Arc::clone(&tool) as Arc<dyn AgentTool>]))
            };

            run_session(&mut server, &factory).await.unwrap();
        });

        initialize(&mut client).await;

        let sender = client.sender();
        let params = PromptParams {
            text: "run approved tool".into(),
            session_id: None,
        };
        let prompt = sender.request::<_, PromptResult>(method::PROMPT, &params);
        let mut prompt = std::pin::pin!(prompt);
        let mut events = Vec::new();
        let mut approvals = 0;
        let result = loop {
            tokio::select! {
                result = &mut prompt => {
                    let result = result.unwrap();
                    while let Some(incoming) = client.try_recv_incoming() {
                        handle_prompt_incoming(incoming, &sender, &mut events, &mut approvals)
                            .await;
                    }
                    break result;
                }
                incoming = client.recv_incoming() => {
                    let incoming = incoming.expect("server should stay connected while prompt runs");
                    handle_prompt_incoming(incoming, &sender, &mut events, &mut approvals).await;
                }
            }
        };

        assert!(!result.turn_id.is_empty());
        assert_eq!(approvals, 1);
        assert!(executed_tool.was_executed());
        assert!(
            events.iter().any(|event| matches!(
                event,
                AgentEvent::ToolApprovalResolved { approved, .. } if *approved
            )),
            "server should continue the turn after receiving approval"
        );

        client
            .sender()
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
            .unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn run_session_rejects_protocol_version_mismatch() {
        let (mut client, mut server) = make_peer_pair();

        let server_task = tokio::spawn(async move {
            run_session(&mut server, &|| Ok(test_agent_options("unused")))
                .await
                .unwrap_err()
        });

        client
            .sender()
            .notify(
                method::INITIALIZE,
                &InitializeParams {
                    protocol_version: "0.9".into(),
                    client: ClientInfo::default(),
                },
            )
            .await
            .unwrap();

        let err = server_task.await.unwrap();
        assert_eq!(err.code, crate::jsonrpc::RpcError::PROTOCOL_MISMATCH);
        assert!(client.try_recv_incoming().is_none());
    }

    #[tokio::test]
    async fn run_session_serves_model_and_thinking_control_requests_between_turns() {
        let (mut client, mut server) = make_peer_pair();

        let server_task = tokio::spawn(async move {
            run_session(&mut server, &|| Ok(test_agent_options("unused")))
                .await
                .unwrap();
        });

        initialize(&mut client).await;
        let sender = client.sender();

        let listed: ModelListResult = sender
            .request(method::MODEL_LIST, &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(listed.current, swink_agent::testing::default_model());
        assert_eq!(
            listed.available,
            vec![swink_agent::testing::default_model()],
            "the primary model is always listed, even with no extra models registered"
        );

        let next = ModelSpec::new("test", "next-model");
        let _: Ack = sender
            .request(method::MODEL_SET, &ModelSetParams::new(next))
            .await
            .unwrap();
        let _: Ack = sender
            .request(
                method::THINKING_SET,
                &ThinkingSetParams::new(ThinkingLevel::High),
            )
            .await
            .unwrap();

        let listed: ModelListResult = sender
            .request(method::MODEL_LIST, &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(listed.current.model_id, "next-model");
        assert_eq!(listed.current.thinking_level, ThinkingLevel::High);

        sender
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
            .unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn run_session_round_trips_approval_mode_and_acks_system_prompt() {
        let (mut client, mut server) = make_peer_pair();

        let server_task = tokio::spawn(async move {
            run_session(&mut server, &|| Ok(test_agent_options("unused")))
                .await
                .unwrap();
        });

        initialize(&mut client).await;
        let sender = client.sender();

        let current: ApprovalGetResult = sender
            .request(method::APPROVAL_GET, &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(current.mode, ApprovalMode::Smart, "Smart is the default");

        let _: Ack = sender
            .request(
                method::APPROVAL_SET,
                &ApprovalSetParams::new(ApprovalMode::Bypassed),
            )
            .await
            .unwrap();

        let current: ApprovalGetResult = sender
            .request(method::APPROVAL_GET, &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(current.mode, ApprovalMode::Bypassed);

        let _: Ack = sender
            .request(
                method::SYSTEM_PROMPT_SET,
                &SystemPromptSetParams::new("you are a replaced prompt"),
            )
            .await
            .unwrap();

        sender
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
            .unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn run_session_rejects_double_plan_enter_and_unpaired_plan_exit() {
        let (mut client, mut server) = make_peer_pair();

        let server_task = tokio::spawn(async move {
            run_session(&mut server, &|| Ok(test_agent_options("unused")))
                .await
                .unwrap();
        });

        initialize(&mut client).await;
        let sender = client.sender();

        let _: Ack = sender
            .request(method::PLAN_ENTER, &serde_json::json!({}))
            .await
            .unwrap();

        let err = sender
            .request::<_, Ack>(method::PLAN_ENTER, &serde_json::json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::RpcError::INVALID_REQUEST);
        assert!(
            err.message.contains("already in plan mode"),
            "unexpected plan.enter error: {}",
            err.message
        );

        let _: Ack = sender
            .request(method::PLAN_EXIT, &serde_json::json!({}))
            .await
            .unwrap();

        let err = sender
            .request::<_, Ack>(method::PLAN_EXIT, &serde_json::json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::RpcError::INVALID_REQUEST);
        assert!(
            err.message.contains("not in plan mode"),
            "unexpected plan.exit error: {}",
            err.message
        );

        sender
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
            .unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn run_session_snapshot_reset_restore_round_trips_messages_and_state() {
        let (mut client, mut server) = make_peer_pair();

        let server_task = tokio::spawn(async move {
            run_session(&mut server, &|| Ok(test_agent_options("snapshot me")))
                .await
                .unwrap();
        });

        initialize(&mut client).await;
        let sender = client.sender();

        // Run one turn so the transcript is non-empty.
        let params = PromptParams {
            text: "hello snapshot".into(),
            session_id: None,
        };
        let result: PromptResult = sender.request(method::PROMPT, &params).await.unwrap();
        assert!(!result.turn_id.is_empty());
        // Drain the buffered agent.event notifications from the turn.
        while client.try_recv_incoming().is_some() {}

        let snapshot: SessionSnapshot = sender
            .request(method::SESSION_SNAPSHOT, &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(
            snapshot.messages.len(),
            2,
            "one user + one assistant message expected"
        );
        // Messages use the memory-JSONL representation: raw LlmMessage JSON.
        let first: LlmMessage = serde_json::from_value(snapshot.messages[0].clone()).unwrap();
        assert!(matches!(first, LlmMessage::User(_)));
        assert!(snapshot.state.is_some());

        let _: Ack = sender
            .request(method::AGENT_RESET, &serde_json::json!({}))
            .await
            .unwrap();
        let cleared: SessionSnapshot = sender
            .request(method::SESSION_SNAPSHOT, &serde_json::json!({}))
            .await
            .unwrap();
        assert!(
            cleared.messages.is_empty(),
            "agent.reset should clear the transcript"
        );

        // Restore the original snapshot, but with explicit session state.
        let restore = SessionSnapshot::new(
            snapshot.messages.clone(),
            Some(serde_json::json!({"data": {"favorite": 42}})),
        );
        let _: Ack = sender
            .request(method::SESSION_RESTORE, &restore)
            .await
            .unwrap();

        let restored: SessionSnapshot = sender
            .request(method::SESSION_SNAPSHOT, &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(restored.messages, snapshot.messages);
        assert_eq!(
            restored.state,
            Some(serde_json::json!({"data": {"favorite": 42}}))
        );

        sender
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
            .unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn run_session_rejects_malformed_session_restore_messages() {
        let (mut client, mut server) = make_peer_pair();

        let server_task = tokio::spawn(async move {
            run_session(&mut server, &|| Ok(test_agent_options("unused")))
                .await
                .unwrap();
        });

        initialize(&mut client).await;
        let sender = client.sender();

        let err = sender
            .request::<_, Ack>(
                method::SESSION_RESTORE,
                &SessionSnapshot::new(vec![serde_json::json!({"not": "a message"})], None),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::RpcError::INVALID_REQUEST);
        assert!(
            err.message.contains("session.restore"),
            "unexpected restore error: {}",
            err.message
        );

        sender
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
            .unwrap();
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn run_prompt_answers_control_requests_with_busy_while_turn_in_flight() {
        let (mut client, mut server) = make_peer_pair();

        let server_task = tokio::spawn(async move {
            run_session(&mut server, &|| Ok(approval_blocking_agent_options()))
                .await
                .unwrap();
        });

        initialize(&mut client).await;
        let sender = client.sender();

        let params = PromptParams {
            text: "start a long prompt".into(),
            session_id: None,
        };
        let prompt_sender = sender.clone();
        let prompt_task = tokio::spawn(async move {
            prompt_sender
                .request::<_, PromptResult>(method::PROMPT, &params)
                .await
        });

        // Wait for the server's tool.approve request — the turn is now
        // provably in flight, blocked on our approval decision.
        let approval_id = loop {
            match client
                .recv_incoming()
                .await
                .expect("server should stay connected while prompt runs")
            {
                IncomingMessage::Request { id, method: m, .. } if m == method::TOOL_APPROVE => {
                    break id;
                }
                IncomingMessage::Notification { method: m, .. } if m == method::AGENT_EVENT => {}
                other => panic!("unexpected message while awaiting tool approval: {other:?}"),
            }
        };

        // Control requests are rejected with BUSY, not dropped and not
        // method_not_found.
        let err = sender
            .request::<_, ModelListResult>(method::MODEL_LIST, &serde_json::json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::jsonrpc::RpcError::BUSY);
        assert!(
            err.message.contains("turn in progress"),
            "unexpected busy error: {}",
            err.message
        );

        // Cancel still works mid-turn; reject the pending approval so the
        // blocked turn unwinds deterministically.
        sender
            .notify(method::CANCEL, &serde_json::Value::Null)
            .await
            .unwrap();
        sender
            .respond_ok(approval_id, ToolApprovalDto::Rejected)
            .await
            .unwrap();

        let result = prompt_task.await.unwrap().unwrap();
        assert!(!result.turn_id.is_empty());
        while client.try_recv_incoming().is_some() {}

        // Between turns the same control request succeeds again.
        let listed: ModelListResult = sender
            .request(method::MODEL_LIST, &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(listed.current, swink_agent::testing::default_model());

        sender
            .notify(method::SHUTDOWN, &serde_json::Value::Null)
            .await
            .unwrap();
        server_task.await.unwrap();
    }

    fn collect_agent_event(incoming: IncomingMessage, events: &mut Vec<AgentEvent>) {
        let IncomingMessage::Notification { method: m, params } = incoming else {
            panic!("unexpected request while collecting prompt events");
        };
        assert_eq!(m, method::AGENT_EVENT);
        let event = serde_json::from_value(params.expect("agent.event should carry params"))
            .expect("agent.event should deserialize");
        events.push(event);
    }

    async fn handle_prompt_incoming(
        incoming: IncomingMessage,
        sender: &crate::jsonrpc::PeerSender,
        events: &mut Vec<AgentEvent>,
        approvals: &mut usize,
    ) {
        match incoming {
            IncomingMessage::Notification { method: m, params } => {
                assert_eq!(m, method::AGENT_EVENT);
                let event =
                    serde_json::from_value(params.expect("agent.event should carry params"))
                        .expect("agent.event should deserialize");
                events.push(event);
            }
            IncomingMessage::Request {
                id,
                method: m,
                params,
            } => {
                assert_eq!(m, method::TOOL_APPROVE);
                let request: ToolApprovalRequestDto =
                    serde_json::from_value(params.expect("tool.approve should carry params"))
                        .expect("tool.approve params should deserialize");
                assert_eq!(request.id, "call-1");
                assert_eq!(request.name, "dangerous_tool");
                assert_eq!(request.arguments["path"], "/tmp/example");
                assert!(request.requires_approval);

                *approvals += 1;
                sender
                    .respond_ok(id, ToolApprovalDto::Approved)
                    .await
                    .unwrap();
            }
        }
    }

    #[cfg(not(unix))]
    #[tokio::test]
    async fn serve_reports_unix_transport_unavailable_on_non_unix_hosts() {
        let server = AgentServer::bind_force("unused.sock", || Ok(test_agent_options("unused")));

        let err = server.serve().await.unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
        assert!(
            err.to_string().contains("Unix socket transport"),
            "unexpected error message: {err}"
        );
    }
}
