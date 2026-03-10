//! Unified event type for the TUI event loop.

use agent_harness::AgentEvent;
use crossterm::event::Event as TerminalEvent;

/// Events that the TUI event loop processes.
#[derive(Debug)]
pub enum AppEvent {
    /// A terminal event (keyboard, mouse, resize).
    Terminal(TerminalEvent),
    /// An event from the agent harness.
    Agent(AgentEvent),
    /// Periodic tick for animations.
    Tick,
}
