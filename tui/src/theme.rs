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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_colors_are_distinct() {
        let colors = [USER_COLOR, ASSISTANT_COLOR, TOOL_COLOR, ERROR_COLOR, THINKING_COLOR];
        for (i, a) in colors.iter().enumerate() {
            for (j, b) in colors.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "role colors at index {i} and {j} should differ");
                }
            }
        }
    }

    #[test]
    fn status_colors_are_distinct() {
        let colors = [STATUS_IDLE, STATUS_RUNNING, STATUS_ERROR, STATUS_ABORTED];
        for (i, a) in colors.iter().enumerate() {
            for (j, b) in colors.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "status colors at index {i} and {j} should differ");
                }
            }
        }
    }

    #[test]
    fn dim_style_has_dim_modifier() {
        let style = dim();
        assert!(style.add_modifier.contains(Modifier::DIM));
    }
}
