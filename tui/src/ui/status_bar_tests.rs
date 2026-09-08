//! Tests for `status_bar`.
#![cfg(test)]

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

/// Acceptance criterion for issue #1084: the status line renders token
/// counts and cost after a stubbed turn.
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
