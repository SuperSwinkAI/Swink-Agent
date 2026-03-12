//! UI layout and rendering.

pub mod conversation;
pub mod diff;
pub mod input;
pub mod markdown;
mod status_bar;
pub mod syntax;
pub mod tool_panel;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};

use crate::app::{App, Focus};

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

    // Conversation
    let conv_area = chunks[idx];
    idx += 1;

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
    app.conversation.render(
        frame,
        conv_area,
        &app.messages,
        app.focus == Focus::Conversation,
        app.blink_on,
        app.selected_tool_block,
    );

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
