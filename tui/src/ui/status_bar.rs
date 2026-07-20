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
    if app.view.show_hidden_channels {
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
mod tests {
    use ratatui::{Terminal, backend::TestBackend};
    use swink_agent::{AgentEvent, AssistantMessage, Cost, StopReason, Usage};

    use crate::app::App;
    use crate::config::TuiConfig;

    /// Render the status bar and flatten it to a single string.
    fn render_to_string(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 1)).expect("test backend");
        terminal
            .draw(|frame| super::render(frame, app, frame.area()))
            .expect("status bar should render");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    /// A stubbed assistant response, priced as the agent loop would have priced
    /// it before the TUI ever saw it.
    fn stubbed_turn(input: u64, output: u64, cost: f64) -> AgentEvent {
        AgentEvent::MessageEnd {
            message: AssistantMessage::new(vec![], "anthropic", "claude-sonnet-4-6")
                .with_usage(Usage::default().with_input(input).with_output(output))
                .with_cost(Cost::default().with_total(cost))
                .with_stop_reason(StopReason::Stop)
                .with_timestamp(0),
        }
    }

    /// Acceptance criterion for issue #1084 / SuperSwink-Coding#134: the status
    /// line renders token counts and cost after a stubbed turn.
    #[test]
    fn status_line_renders_token_counts_after_a_stubbed_turn() {
        let mut app = App::new(TuiConfig::default());
        app.handle_agent_event(stubbed_turn(1_200, 340, 0.0042));

        let rendered = render_to_string(&app);
        assert!(rendered.contains("↓1.2K ↑340"), "{rendered}");
        assert!(rendered.contains("$0.0042"), "{rendered}");
        assert!(rendered.contains("claude-sonnet-4-6"), "{rendered}");
    }

    #[test]
    fn status_line_accumulates_across_turns() {
        let mut app = App::new(TuiConfig::default());
        app.handle_agent_event(stubbed_turn(100, 20, 0.01));
        app.handle_agent_event(stubbed_turn(200, 30, 0.02));

        let rendered = render_to_string(&app);
        assert!(rendered.contains("↓300 ↑50"), "{rendered}");
        assert!(rendered.contains("$0.0300"), "{rendered}");
    }

    /// A model with neither catalog nor operator pricing accrues no cost; the
    /// status line must still render, showing an honest zero.
    #[test]
    fn status_line_renders_zero_cost_for_an_unpriced_model() {
        let mut app = App::new(TuiConfig::default());
        app.handle_agent_event(stubbed_turn(500, 100, 0.0));

        let rendered = render_to_string(&app);
        assert!(rendered.contains("↓500 ↑100"), "{rendered}");
        assert!(rendered.contains("$0.0000"), "{rendered}");
    }

    #[test]
    fn status_line_renders_before_any_turn() {
        let rendered = render_to_string(&App::new(TuiConfig::default()));
        assert!(rendered.contains("IDLE"), "{rendered}");
        assert!(rendered.contains("↓0 ↑0"), "{rendered}");
        assert!(rendered.contains("$0.0000"), "{rendered}");
    }
}
