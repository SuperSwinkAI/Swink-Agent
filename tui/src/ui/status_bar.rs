//! Status bar rendering.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{AgentStatus, App, OperatingMode};
use crate::format;
use crate::theme;
use crate::theme::ColorMode;

/// Render the status bar.
pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let (status_text, status_color) = match app.agent_io.status {
        AgentStatus::Idle => ("IDLE", theme::status_idle()),
        AgentStatus::Running => ("RUNNING", theme::status_running()),
        AgentStatus::Error => ("ERROR", theme::status_error()),
        AgentStatus::Aborted => ("ABORTED", theme::status_aborted()),
    };

    let elapsed = format::format_elapsed(app.session.session_start);
    let input_tokens = format::format_tokens(app.usage.total_input_tokens);
    let output_tokens = format::format_tokens(app.usage.total_output_tokens);

    let mut spans = vec![Span::styled(
        format!(" {status_text} "),
        Style::default().fg(theme::bar_bg()).bg(status_color),
    )];

    // Operating mode badge (only in Plan mode)
    if app.mode.operating_mode == OperatingMode::Plan {
        spans.push(Span::styled(
            " PLAN ",
            Style::default().fg(theme::bar_bg()).bg(theme::plan_color()),
        ));
    }

    // Hidden-channels badge (only when the toggle is on)
    if app.view.conversation.show_hidden_channels {
        spans.push(Span::styled(
            " HIDDEN ",
            Style::default()
                .fg(theme::bar_bg())
                .bg(theme::thinking_color()),
        ));
    }

    // Color mode badge (only when not Custom)
    let mode = theme::color_mode();
    if mode != ColorMode::Custom {
        let label = match mode {
            ColorMode::MonoWhite => " MONO-W ",
            ColorMode::MonoBlack => " MONO-B ",
            ColorMode::Custom => unreachable!(),
        };
        spans.push(Span::styled(
            label,
            Style::default().fg(theme::bar_bg()).bg(theme::bar_fg()),
        ));
    }

    spans.extend([
        Span::raw("  "),
        Span::styled(&app.mode.model_name, theme::dim()),
        Span::raw("  │  "),
        Span::raw(format!("↓{input_tokens} ↑{output_tokens}")),
        Span::raw("  │  "),
        Span::raw(format!("${:.4}", app.usage.total_cost)),
        Span::raw("  │  "),
        Span::styled(elapsed, theme::dim()),
    ]);

    // Context window gauge
    if app.usage.context_budget > 0 {
        let (gauge, pct) =
            format::format_context_gauge(app.usage.context_tokens_used, app.usage.context_budget);
        let gauge_color = if pct < 60.0 {
            theme::context_green()
        } else if pct < 85.0 {
            theme::context_yellow()
        } else {
            theme::context_red()
        };
        spans.push(Span::raw("  │  "));
        spans.push(Span::styled(gauge, Style::default().fg(gauge_color)));
        spans.push(Span::raw(format!(" {pct:.0}%")));
    }

    // Show retry indicator
    if let Some(attempt) = app.agent_io.retry_attempt {
        spans.push(Span::raw("  │  "));
        spans.push(Span::styled(
            format!("Retrying... (attempt {attempt})"),
            Style::default().fg(theme::error_color()),
        ));
    }

    let status = Line::from(spans);
    let bar =
        Paragraph::new(status).style(Style::default().bg(theme::bar_bg()).fg(theme::bar_fg()));
    frame.render_widget(bar, area);
}

#[cfg(test)]
#[path = "status_bar_tests.rs"]
mod tests;
