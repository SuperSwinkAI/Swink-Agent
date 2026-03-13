//! Scrollable conversation view with role-colored message blocks.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

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

    /// Clamp the current scroll offset to the rendered content.
    pub const fn clamp_scroll_offset(&mut self, visible_height: usize) {
        let max = self.rendered_lines.saturating_sub(visible_height);
        if self.scroll_offset > max {
            self.scroll_offset = max;
        }
    }

    #[cfg(test)]
    pub(crate) const fn set_rendered_lines_for_test(&mut self, rendered_lines: usize) {
        self.rendered_lines = rendered_lines;
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
    #[allow(clippy::too_many_lines)]
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        messages: &[DisplayMessage],
        focused: bool,
        blink_on: bool,
        selected_tool_block: Option<usize>,
    ) {
        let border_color = if focused {
            theme::border_focused_color()
        } else {
            theme::border_color()
        };

        let inner_width = area.width.saturating_sub(2); // account for borders
        let inner_height = area.height.saturating_sub(2) as usize; // account for borders

        // Build all lines from messages
        let mut all_lines: Vec<Line<'static>> = Vec::new();

        for (msg_idx, msg) in messages.iter().enumerate() {
            let (role_label, role_color) = match msg.role {
                MessageRole::User => ("You", theme::user_color()),
                MessageRole::Assistant => {
                    if msg.plan_mode {
                        ("Plan", theme::plan_color())
                    } else {
                        ("Assistant", theme::assistant_color())
                    }
                }
                MessageRole::ToolResult => ("Tool", theme::tool_color()),
                MessageRole::Error => ("Error", theme::error_color()),
                MessageRole::System => ("System", theme::system_color()),
            };

            // Collapsed tool result: show one-line summary
            if msg.role == MessageRole::ToolResult && msg.collapsed {
                let is_selected = selected_tool_block == Some(msg_idx);
                let select_style = if is_selected {
                    Style::default().fg(role_color).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(role_color)
                };
                all_lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(role_color)),
                    Span::styled("▶ ", select_style),
                    Span::styled(
                        role_label.to_string(),
                        Style::default().fg(role_color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  ", Style::default()),
                    Span::styled(msg.summary.clone(), theme::dim()),
                    Span::styled(" [F2]", theme::dim()),
                ]));
                all_lines.push(Line::from(""));
                continue;
            }

            // Expanded tool result: show ▼ indicator
            let indicator = if msg.role == MessageRole::ToolResult {
                let is_selected = selected_tool_block == Some(msg_idx);
                let select_style = if is_selected {
                    Style::default().fg(role_color).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(role_color)
                };
                Some(Span::styled("▼ ", select_style))
            } else {
                None
            };

            // Role header line with colored left border
            let mut header_spans = vec![Span::styled("│ ", Style::default().fg(role_color))];
            if let Some(ind) = indicator {
                header_spans.push(ind);
            }
            header_spans.push(Span::styled(
                role_label.to_string(),
                Style::default().fg(role_color).add_modifier(Modifier::BOLD),
            ));
            all_lines.push(Line::from(header_spans));

            // Thinking section (dimmed, collapsed)
            if let Some(thinking) = &msg.thinking
                && !thinking.is_empty()
            {
                let thinking_style = Style::default()
                    .fg(theme::thinking_color())
                    .add_modifier(Modifier::DIM);
                all_lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(role_color)),
                    Span::styled("💭 ", thinking_style),
                    Span::styled("[thinking...]", thinking_style),
                ]));
            }

            // Message content with markdown rendering
            let content_lines =
                markdown::markdown_to_lines(&msg.content, inner_width.saturating_sub(2));
            for line in content_lines {
                let mut spans = vec![Span::styled("│ ", Style::default().fg(role_color))];
                spans.extend(line.spans);
                all_lines.push(Line::from(spans));
            }

            // Render diff view for file modifications
            if msg.role == MessageRole::ToolResult
                && let Some(ref diff) = msg.diff_data
            {
                let diff_lines = crate::ui::diff::render_diff_lines(diff, inner_width);
                for line in diff_lines {
                    let mut spans = vec![Span::styled("│ ", Style::default().fg(role_color))];
                    spans.extend(line.spans);
                    all_lines.push(Line::from(spans));
                }
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
        self.clamp_scroll_offset(inner_height);

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

#[cfg(test)]
mod tests {
    use super::ConversationView;

    #[test]
    fn scroll_up_disengages_auto_scroll() {
        let mut view = ConversationView::new();
        view.scroll_offset = 5;
        view.scroll_up(2);

        assert_eq!(view.scroll_offset, 3);
        assert!(!view.auto_scroll);
    }

    #[test]
    fn scroll_down_to_bottom_reengages_auto_scroll() {
        let mut view = ConversationView::new();
        view.set_rendered_lines_for_test(30);
        view.scroll_offset = 20;
        view.auto_scroll = false;

        view.scroll_down(10, 10);

        assert_eq!(view.scroll_offset, 20);
        assert!(view.auto_scroll);
    }

    #[test]
    fn clamp_scroll_offset_uses_visible_height() {
        let mut view = ConversationView::new();
        view.set_rendered_lines_for_test(25);
        view.scroll_offset = 99;

        view.clamp_scroll_offset(8);

        assert_eq!(view.scroll_offset, 17);
    }
}
