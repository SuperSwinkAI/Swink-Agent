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
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Focus};
use crate::ui::help_panel::MIN_CONV_WIDTH;

/// Minimum terminal width for normal UI rendering.
pub const MIN_TERMINAL_WIDTH: u16 = 120;
/// Minimum terminal height for normal UI rendering.
pub const MIN_TERMINAL_HEIGHT: u16 = 30;

/// Returns true if the terminal dimensions meet the minimum size requirements.
pub const fn meets_minimum_size(width: u16, height: u16) -> bool {
    width >= MIN_TERMINAL_WIDTH && height >= MIN_TERMINAL_HEIGHT
}

/// Render the complete UI into the given frame.
pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if !meets_minimum_size(area.width, area.height) {
        render_size_warning(frame, area.width, area.height);
        return;
    }
    let input_height = app.input.height();
    let mut tool_height = app.tool_panel.height();
    // Account for extra prompt lines (plan approval, trust follow-up).
    let has_trust_follow_up = app.trust_follow_up.is_some();
    let has_plan_approval = app.pending_plan_approval;
    if has_plan_approval {
        tool_height = tool_height.max(3) + 1; // at least borders + 1 line
    }
    if has_trust_follow_up {
        tool_height = tool_height.max(3) + 1;
    }

    // Queued-message overlay: visible while pending_steered is non-empty or
    // fading out after AgentEnd. Height = 1 line per queued message + 2 borders,
    // capped at 5 visible lines total.
    let steered_visible = !app.pending_steered.is_empty() || app.steered_fade_ticks > 0;
    let steered_height: u16 = if steered_visible {
        let lines = u16::try_from(app.pending_steered.len().max(1)).unwrap_or(u16::MAX);
        (lines + 2).min(7)
    } else {
        0
    };

    let mut constraints = vec![Constraint::Min(3)]; // Conversation view

    if tool_height > 0 {
        constraints.push(Constraint::Length(tool_height));
    }
    if steered_height > 0 {
        constraints.push(Constraint::Length(steered_height));
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

    // Steered message overlay (conditional)
    let steered_area = if steered_height > 0 {
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
        app.selection.as_ref(),
    );

    // Render help panel
    if let Some(area) = help_area {
        app.help_panel.render(frame, area);
    }

    // Render tool panel
    if let Some(area) = tool_area {
        let trust_name = app.trust_follow_up.as_ref().map(|f| f.tool_name.as_str());
        app.tool_panel
            .render_with_prompts(frame, area, app.pending_plan_approval, trust_name);
    }

    // Render steered message overlay
    if let Some(area) = steered_area {
        render_steered_overlay(frame, area, &app.pending_steered, app.steered_fade_ticks);
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

/// Render the queued-message overlay shown while steered messages are in flight
/// or fading out after they have been consumed by the agent.
///
/// - `pending`: messages currently waiting to be processed (non-empty = queued).
/// - `fade_ticks`: remaining ticks of the fade-out animation (0 = fully gone).
fn render_steered_overlay(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    pending: &[String],
    fade_ticks: u8,
) {
    // Fading = steered messages have just been consumed; show dimmed until gone.
    let fading = pending.is_empty() && fade_ticks > 0;

    let border_color = if fading {
        Color::DarkGray
    } else {
        Color::Yellow
    };

    let label_style = if fading {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    } else {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    };

    let content_style = if fading {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    } else {
        Style::default().fg(Color::White)
    };

    let title = if fading { " Sent " } else { " Queued " };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(title, label_style))
        .border_style(Style::default().fg(border_color));

    let messages: Vec<Line> = if fading {
        vec![Line::from(Span::styled(
            "↑ delivered to agent",
            content_style,
        ))]
    } else {
        pending
            .iter()
            .map(|msg| {
                let preview: String = msg.chars().take(120).collect();
                Line::from(vec![
                    Span::styled("⏳ ", label_style),
                    Span::styled(preview, content_style),
                ])
            })
            .collect()
    };

    let paragraph = Paragraph::new(messages).block(block);
    frame.render_widget(paragraph, area);
}

/// Render a centered warning when the terminal is below minimum size.
fn render_size_warning(frame: &mut Frame, width: u16, height: u16) {
    let area = frame.area();
    frame.render_widget(Clear, area);

    let lines = vec![
        Line::from(Span::styled(
            "Terminal Too Small",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!("Current size: {width} x {height}")),
        Line::from(format!(
            "Minimum required: {MIN_TERMINAL_WIDTH} x {MIN_TERMINAL_HEIGHT}"
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Please resize your terminal to continue.",
            Style::default().add_modifier(Modifier::DIM),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Swink Agent ")
        .border_style(Style::default().fg(Color::Yellow));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .alignment(Alignment::Center);

    frame.render_widget(paragraph, area);
}
