//! UI layout and rendering.

pub mod conversation;
pub mod diff;
pub mod help_panel;
pub mod input;
pub mod markdown;
mod status_bar;
pub mod syntax;
pub mod tool_panel;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use crate::app::{App, Focus};
use crate::ui::help_panel::MIN_CONV_WIDTH;

/// Render the complete UI into the given frame.
pub fn render(frame: &mut Frame, app: &mut App) {
    let input_height = app.input.height();
    let tool_height = app.tool_panel.height();

    let mut constraints = vec![Constraint::Min(3)]; // Conversation view

    if tool_height > 0 {
        constraints.push(Constraint::Length(tool_height));
    }
    constraints.push(Constraint::Length(input_height)); // Input editor
    constraints.push(Constraint::Length(1)); // Status bar

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    let mut idx = 0;

    // Conversation — optionally split horizontally for help panel
    let full_conv_area = chunks[idx];
    idx += 1;

    let help_width = app.help_panel.width();
    let (conv_area, help_area) =
        if help_width > 0 && full_conv_area.width >= help_width + MIN_CONV_WIDTH {
            let h_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Min(MIN_CONV_WIDTH),
                    Constraint::Length(help_width),
                ])
                .split(full_conv_area);
            (h_chunks[0], Some(h_chunks[1]))
        } else {
            (full_conv_area, None)
        };

    // Tool panel (conditional)
    let tool_area = if tool_height > 0 {
        let area = chunks[idx];
        idx += 1;
        Some(area)
    } else {
        None
    };

    // Input
    let input_area = chunks[idx];
    idx += 1;

    // Status bar
    let status_area = chunks[idx];

    // Render conversation
    app.conversation_area = conv_area;
    app.conversation_visible_height = conv_area.height.saturating_sub(2) as usize;
    app.conversation.render(
        frame,
        conv_area,
        &app.messages,
        app.focus == Focus::Conversation,
        app.blink_on,
        app.selected_tool_block,
    );

    // Render help panel
    if let Some(area) = help_area {
        app.help_panel.render(frame, area);
    }

    // Render tool panel
    if let Some(area) = tool_area {
        app.tool_panel.render(frame, area);
    }

    // Render input
    let status_hint = match app.status {
        crate::app::AgentStatus::Running => "running...",
        crate::app::AgentStatus::Error => "error",
        crate::app::AgentStatus::Aborted => "aborted",
        crate::app::AgentStatus::Idle => "",
    };
    app.input
        .render(frame, input_area, app.focus == Focus::Input, status_hint);

    // Render status bar
    status_bar::render(frame, app, status_area);
}
