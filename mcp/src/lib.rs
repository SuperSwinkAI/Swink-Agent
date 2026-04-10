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
//! let configs = vec![McpServerConfig {
//!     name: "filesystem".into(),
//!     transport: McpTransport::Stdio {
//!         command: "npx".into(),
//!         args: vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into()],
//!         env: Default::default(),
//!     },
//!     tool_prefix: Some("fs".into()),
//!     tool_filter: None,
//!     requires_approval: true,
//! }];
//!
//! let mut mcp = McpManager::new(configs);
//! mcp.connect_all().await?;
//! let tools = mcp.tools();
//! ```
mod config;
mod connection;
pub mod convert;
mod error;
pub mod event;
mod manager;
mod tool;

pub use config::{McpServerConfig, McpTransport, ToolFilter};
pub use connection::{McpConnection, McpConnectionStatus};
pub use error::McpError;
pub use manager::McpManager;
pub use tool::McpTool;
