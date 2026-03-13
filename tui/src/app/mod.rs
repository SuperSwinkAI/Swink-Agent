//! Top-level application state and event loop.

mod agent_bridge;
mod event_loop;
mod lifecycle;
mod persistence;
mod render_helpers;
mod state;

pub use state::{AgentStatus, App, DisplayMessage, Focus, MessageRole, OperatingMode};

pub(crate) type AppResult<T> = Result<T, Box<dyn std::error::Error>>;

/// Seconds before a tool result auto-collapses (unless user-expanded).
pub(crate) const AUTO_COLLAPSE_SECS: u64 = 10;
/// Mouse wheel scroll distance in rendered lines.
pub(crate) const MOUSE_SCROLL_STEP: usize = 3;
/// Maximum number of visible turns retained in the TUI display model.
pub(crate) const MAX_VISIBLE_TURNS: usize = 20;

#[cfg(test)]
mod tests;
