//! Status bar rendering.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{AgentStatus, App};
use crate::theme;

/// Render the status bar.
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let (status_text, status_color) = match app.status {
        AgentStatus::Idle => ("IDLE", theme::STATUS_IDLE),
        AgentStatus::Running => ("RUNNING", theme::STATUS_RUNNING),
        AgentStatus::Error => ("ERROR", theme::STATUS_ERROR),
        AgentStatus::Aborted => ("ABORTED", theme::STATUS_ABORTED),
    };

    let status = Line::from(vec![
        Span::styled(
            format!(" {status_text} "),
            Style::default()
                .fg(ratatui::style::Color::Black)
                .bg(status_color),
        ),
        Span::raw("  "),
        Span::styled(&app.model_name, theme::dim()),
        Span::raw("  |  "),
        Span::raw(format!(
            "tokens: {}down {}up",
            app.total_input_tokens, app.total_output_tokens
        )),
        Span::raw("  |  "),
        Span::raw(format!("${:.4}", app.total_cost)),
    ]);

    let bar = Paragraph::new(status).style(Style::default().bg(ratatui::style::Color::DarkGray));
    frame.render_widget(bar, area);
}
