//! Color palette and style constants for the TUI.

use ratatui::style::{Color, Modifier, Style};

/// Colors for different message roles.
pub const USER_COLOR: Color = Color::Green;
pub const ASSISTANT_COLOR: Color = Color::Cyan;
pub const TOOL_COLOR: Color = Color::Yellow;
pub const ERROR_COLOR: Color = Color::Red;
pub const THINKING_COLOR: Color = Color::DarkGray;

/// Status indicator colors.
pub const STATUS_IDLE: Color = Color::Green;
pub const STATUS_RUNNING: Color = Color::Yellow;
pub const STATUS_ERROR: Color = Color::Red;
pub const STATUS_ABORTED: Color = Color::Magenta;

/// Dimmed style helper.
pub fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}
