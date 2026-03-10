//! Status bar rendering.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{AgentStatus, App};
use crate::format;
use crate::theme;

/// Render the status bar.
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let (status_text, status_color) = match app.status {
        AgentStatus::Idle => ("IDLE", theme::STATUS_IDLE),
        AgentStatus::Running => ("RUNNING", theme::STATUS_RUNNING),
        AgentStatus::Error => ("ERROR", theme::STATUS_ERROR),
        AgentStatus::Aborted => ("ABORTED", theme::STATUS_ABORTED),
    };

    let elapsed = format::format_elapsed(app.session_start);
    let input_tokens = format::format_tokens(app.total_input_tokens);
    let output_tokens = format::format_tokens(app.total_output_tokens);

    let mut spans = vec![
        Span::styled(
            format!(" {status_text} "),
            Style::default()
                .fg(ratatui::style::Color::Black)
                .bg(status_color),
        ),
        Span::raw("  "),
        Span::styled(&app.model_name, theme::dim()),
        Span::raw("  │  "),
        Span::raw(format!("↓{input_tokens} ↑{output_tokens}")),
        Span::raw("  │  "),
        Span::raw(format!("${:.4}", app.total_cost)),
        Span::raw("  │  "),
        Span::styled(elapsed, theme::dim()),
    ];

    // Show retry indicator
    if let Some(attempt) = app.retry_attempt {
        spans.push(Span::raw("  │  "));
        spans.push(Span::styled(
            format!("Retrying... (attempt {attempt})"),
            Style::default().fg(theme::ERROR_COLOR),
        ));
    }

    let status = Line::from(spans);
    let bar = Paragraph::new(status).style(Style::default().bg(ratatui::style::Color::DarkGray));
    frame.render_widget(bar, area);
}
