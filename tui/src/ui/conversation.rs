//! Scrollable conversation view with role-colored message blocks.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{DisplayMessage, MessageRole};
use crate::theme;
use crate::ui::markdown;

/// Conversation view state.
pub struct ConversationView {
    /// Current scroll offset in lines.
    pub scroll_offset: usize,
    /// Whether auto-scroll is engaged.
    pub auto_scroll: bool,
    /// Total rendered lines (computed each frame).
    rendered_lines: usize,
}

impl ConversationView {
    pub const fn new() -> Self {
        Self {
            scroll_offset: 0,
            auto_scroll: true,
            rendered_lines: 0,
        }
    }

    /// Scroll up by `n` lines. Disengages auto-scroll.
    pub const fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        self.auto_scroll = false;
    }

    /// Scroll down by `n` lines. Re-engages auto-scroll if at bottom.
    pub const fn scroll_down(&mut self, n: usize, visible_height: usize) {
        self.scroll_offset += n;
        let max = self.rendered_lines.saturating_sub(visible_height);
        if self.scroll_offset >= max {
            self.scroll_offset = max;
            self.auto_scroll = true;
        }
    }

    /// Scroll to bottom and re-engage auto-scroll.
    ///
    /// Reserved for future use by keyboard shortcut (e.g. Ctrl+End).
    #[allow(dead_code)]
    pub const fn scroll_to_bottom(&mut self, visible_height: usize) {
        let max = self.rendered_lines.saturating_sub(visible_height);
        self.scroll_offset = max;
        self.auto_scroll = true;
    }

    /// Render the conversation view.
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        messages: &[DisplayMessage],
        focused: bool,
        blink_on: bool,
    ) {
        let border_color = if focused {
            Color::White
        } else {
            Color::DarkGray
        };

        let inner_width = area.width.saturating_sub(2); // account for borders
        let inner_height = area.height.saturating_sub(2) as usize; // account for borders

        // Build all lines from messages
        let mut all_lines: Vec<Line<'static>> = Vec::new();

        for msg in messages {
            let (role_label, role_color) = match msg.role {
                MessageRole::User => ("You", theme::USER_COLOR),
                MessageRole::Assistant => ("Assistant", theme::ASSISTANT_COLOR),
                MessageRole::ToolResult => ("Tool", theme::TOOL_COLOR),
                MessageRole::Error => ("Error", theme::ERROR_COLOR),
                MessageRole::System => ("System", Color::Magenta),
            };

            // Role header line with colored left border
            all_lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(role_color)),
                Span::styled(
                    role_label.to_string(),
                    Style::default()
                        .fg(role_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));

            // Thinking section (dimmed, collapsed)
            if let Some(thinking) = &msg.thinking {
                if !thinking.is_empty() {
                    let thinking_style = Style::default()
                        .fg(theme::THINKING_COLOR)
                        .add_modifier(Modifier::DIM);
                    all_lines.push(Line::from(vec![
                        Span::styled("│ ", Style::default().fg(role_color)),
                        Span::styled("💭 ", thinking_style),
                        Span::styled("[thinking...]", thinking_style),
                    ]));
                }
            }

            // Message content with markdown rendering
            let content_lines = markdown::markdown_to_lines(&msg.content, inner_width.saturating_sub(2));
            for line in content_lines {
                let mut spans = vec![Span::styled("│ ", Style::default().fg(role_color))];
                spans.extend(line.spans);
                all_lines.push(Line::from(spans));
            }

            // Streaming cursor
            if msg.is_streaming && blink_on {
                all_lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(role_color)),
                    Span::styled("█", Style::default().fg(role_color)),
                ]));
            }

            // Blank line between messages
            all_lines.push(Line::from(""));
        }

        self.rendered_lines = all_lines.len();

        // Auto-scroll: jump to bottom
        if self.auto_scroll {
            let max = self.rendered_lines.saturating_sub(inner_height);
            self.scroll_offset = max;
        }

        // Clamp scroll offset
        let max_scroll = self.rendered_lines.saturating_sub(inner_height);
        self.scroll_offset = self.scroll_offset.min(max_scroll);

        // Build title with scroll indicator
        let title = if !self.auto_scroll && self.scroll_offset < max_scroll {
            " Conversation  ↓ scroll to bottom "
        } else {
            " Conversation "
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(border_color));

        let paragraph = Paragraph::new(all_lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((u16::try_from(self.scroll_offset).unwrap_or(u16::MAX), 0));

        frame.render_widget(paragraph, area);
    }
}
