//! JSON-RPC 2.0 agent service for `swink-agent`.
//!
//! Exposes an [`Agent`](swink_agent::Agent) over a Unix-domain socket using
//! newline-delimited JSON-RPC 2.0. Bidirectional: the server streams
//! `AgentEvent` notifications and sends `tool.approve` requests; the client
//! drives turns via `prompt` requests and steers the agent through the
//! control-plane requests below.
//!
//! # Protocol
//!
//! Protocol version [`dto::PROTOCOL_VERSION`]. The handshake is a pair of
//! notifications: the client sends `initialize`
//! ([`InitializeParams`](dto::InitializeParams)), the server replies with
//! `initialized` ([`InitializedParams`](dto::InitializedParams)).
//!
//! Client→server requests:
//!
//! | Method | Params | Result |
//! |---|---|---|
//! | `prompt` | [`PromptParams`](dto::PromptParams) | [`PromptResult`](dto::PromptResult) |
//! | `model.list` | `{}` | [`ModelListResult`](dto::ModelListResult) |
//! | `model.set` | [`ModelSetParams`](dto::ModelSetParams) | [`Ack`](dto::Ack) |
//! | `thinking.set` | [`ThinkingSetParams`](dto::ThinkingSetParams) | [`Ack`](dto::Ack) |
//! | `approval.get` | `{}` | [`ApprovalGetResult`](dto::ApprovalGetResult) |
//! | `approval.set` | [`ApprovalSetParams`](dto::ApprovalSetParams) | [`Ack`](dto::Ack) |
//! | `system_prompt.set` | [`SystemPromptSetParams`](dto::SystemPromptSetParams) | [`Ack`](dto::Ack) |
//! | `agent.reset` | `{}` | [`Ack`](dto::Ack) |
//! | `plan.enter` | `{}` | [`Ack`](dto::Ack) |
//! | `plan.exit` | `{}` | [`Ack`](dto::Ack) |
//! | `session.snapshot` | `{}` | [`SessionSnapshot`](dto::SessionSnapshot) |
//! | `session.restore` | [`SessionSnapshot`](dto::SessionSnapshot) | [`Ack`](dto::Ack) |
//!
//! Everything below `prompt` is a control-plane request (see
//! [`dto::method::is_control`]): it is served between turns, and answered
//! with [`RpcError::BUSY`](jsonrpc::RpcError::BUSY) while a turn is in
//! flight. The `cancel` notification is the mid-turn-safe way to abort a
//! running turn; `shutdown` (notification) ends the session. The server
//! sends `agent.event` notifications and `tool.approve`
//! ([`ToolApprovalRequestDto`](dto::ToolApprovalRequestDto) →
//! [`ToolApprovalDto`](dto::ToolApprovalDto)) requests to the client.
//!
//! # Quick start
//!
//! **Server:**
//! ```no_run
//! # #[cfg(unix)]
//! # async fn run() -> std::io::Result<()> {
//! use swink_agent_rpc::AgentServer;
//!
//! AgentServer::bind("/tmp/swink.sock", || {
//!     // build AgentOptions from env / keyring
//!     # unimplemented!()
//! })?.serve().await
//! # }
//! ```
//!
//! **Client:**
//! ```no_run
//! # #[cfg(unix)]
//! # async fn run() -> Result<(), swink_agent_rpc::jsonrpc::RpcError> {
//! use swink_agent_rpc::AgentClient;
//!
//! let mut client = AgentClient::connect("/tmp/swink.sock").await?;
//! let events = client.prompt_text("Hello!").await?;
//! println!("{} events received", events.len());
//! # Ok(())
//! # }
//! ```
//!
//! # Platform support
//!
//! Unix only. The transport is a Unix domain socket whose security model rests
//! on two POSIX mechanisms: the socket file is created with mode `0600`, and
//! every accepted connection is checked against the peer's user id via
//! `SO_PEERCRED` (Linux) or `getpeereid` (macOS and the BSDs). Neither has a
//! drop-in Windows equivalent, so a named-pipe port would be a second
//! transport rather than a conditional branch.
//!
//! The crate still compiles on Windows. `AgentServer::serve` returns
//! [`std::io::ErrorKind::Unsupported`] and `AgentClient::connect` returns the
//! matching [`RpcError`](jsonrpc::RpcError); both carry the message "Unix
//! socket transport requires a Unix host". See `docs/platform-support.md`.

#![forbid(unsafe_code)]

pub mod dto;
pub mod jsonrpc;

#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub use server::AgentServer;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub use client::AgentClient;
