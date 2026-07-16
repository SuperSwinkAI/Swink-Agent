#![forbid(unsafe_code)]
//! MCP (Model Context Protocol) integration for swink-agent.
//!
//! Provides connection management, tool discovery, and tool execution for
//! MCP servers. Discovered tools implement the `AgentTool` trait and can
//! be used alongside native tools in the agent loop.
//!
//! # Quick start
//!
//! ```ignore
//! use swink_agent_mcp::{McpManager, McpServerConfig, McpTransport};
//!
//! let configs = vec![
//!     McpServerConfig::new(
//!         "filesystem",
//!         McpTransport::Stdio {
//!             command: "npx".into(),
//!             args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into()],
//!             env: Default::default(),
//!         },
//!     )
//!     .with_tool_prefix("fs"),
//! ];
//!
//! let mut mcp = McpManager::new(configs);
//! mcp.connect_all().await?;
//! let tools = mcp.tools();
//! ```
/// Ensure a process-wide default rustls crypto provider is installed.
///
/// The workspace builds reqwest with `rustls-no-provider` (#1110), so a
/// `reqwest::Client` cannot be constructed — including the one rmcp's
/// streamable-HTTP transport builds internally — until a process default
/// [`rustls::crypto::CryptoProvider`] exists. Installs ring; idempotent —
/// an already-installed provider (e.g. a host's aws-lc-rs for FIPS) wins.
pub(crate) fn ensure_default_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

mod config;
mod connection;
pub mod convert;
mod error;
pub mod event;
mod manager;
mod tool;

pub use config::{McpServerConfig, McpTransport, SseBearerAuth, ToolFilter};
pub use connection::{McpConnection, McpConnectionStatus};
pub use error::McpError;
pub use manager::McpManager;
pub use tool::McpTool;
