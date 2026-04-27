//! JSON-RPC 2.0 agent service for `swink-agent`.
//!
//! Exposes an [`Agent`](swink_agent::Agent) over a Unix-domain socket using
//! newline-delimited JSON-RPC 2.0. Bidirectional: the server streams
//! `AgentEvent` notifications and sends `tool.approve` requests; the client
//! drives turns via `prompt` requests.
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
