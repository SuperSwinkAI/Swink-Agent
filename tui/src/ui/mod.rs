//! UI layout and rendering.

mod status_bar;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use crate::app::{App, MessageRole};
use crate::theme;

/// Render the complete UI into the given frame.
pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // Conversation view
            Constraint::Length(3), // Input editor
            Constraint::Length(1), // Status bar
        ])
        .split(frame.area());

    render_conversation(frame, app, chunks[0]);
    render_input(frame, app, chunks[1]);
    status_bar::render(frame, app, chunks[2]);
}

/// Render the conversation message history.
fn render_conversation(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = app
        .messages
        .iter()
        .map(|msg| {
            let (prefix, style) = match msg.role {
                MessageRole::User => ("You", theme::user_style()),
                MessageRole::Assistant => ("Assistant", theme::assistant_style()),
                MessageRole::ToolResult => ("Tool", Style::default().fg(theme::TOOL_COLOR)),
                MessageRole::Error => ("Error", theme::error_style()),
            };
            let line = Line::from(vec![
                Span::styled(format!("{prefix}: "), style),
                Span::raw(&msg.content),
            ]);
            ListItem::new(line)
        })
        .collect();

    let conversation = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Conversation "));

    frame.render_widget(conversation, area);
}

/// Render the input editor.
fn render_input(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let input = Paragraph::new(app.input.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Message ")
                .border_style(Style::default().fg(theme::ACCENT)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(input, area);

    // Position cursor at the end of input.
    let cursor_x = area.x + 1 + app.input.len() as u16;
    let cursor_y = area.y + 1;
    frame.set_cursor_position((cursor_x.min(area.x + area.width - 2), cursor_y));
}
