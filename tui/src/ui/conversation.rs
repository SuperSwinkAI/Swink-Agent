//! Scrollable conversation view with role-colored message blocks.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{DisplayMessage, MessageRole, Selection};
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
    /// Per-row cell symbols captured from the last render pass, indexed
    /// `[row][col]` over the conversation's inner area. Used by selection
    /// copy to extract exactly what the user sees on screen.
    pub(crate) visible_cells: Vec<Vec<String>>,
}

impl Default for ConversationView {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationView {
    pub const fn new() -> Self {
        Self {
            scroll_offset: 0,
            auto_scroll: true,
            rendered_lines: 0,
            visible_cells: Vec::new(),
        }
    }

    /// Extract the text inside `selection` from the last captured render.
    /// Returns the selected text with a leading `"│ "` gutter stripped per
    /// line and trailing whitespace removed. Returns `None` if the selection
    /// is empty or out of bounds.
    pub(crate) fn selection_text(&self, selection: &Selection) -> Option<String> {
        if selection.is_empty() || self.visible_cells.is_empty() {
            return None;
        }
        let (start, end) = selection.normalized();
        let start_row = start.0 as usize;
        let end_row = end.0 as usize;
        let mut out = String::new();
        for row_idx in start_row..=end_row {
            let Some(row) = self.visible_cells.get(row_idx) else {
                break;
            };
            let width = row.len();
            let col_start = if row_idx == start_row {
                start.1 as usize
            } else {
                0
            };
            let col_end = if row_idx == end_row {
                end.1 as usize
            } else {
                width
            };
            let col_start = col_start.min(width);
            let col_end = col_end.min(width).max(col_start);
            let mut line: String = row[col_start..col_end].concat();
            if line.starts_with("│ ") {
                line.drain(.."│ ".len());
            }
            let trimmed = line.trim_end();
            out.push_str(trimmed);
            if row_idx != end_row {
                out.push('\n');
            }
        }
        let trimmed = out.trim_matches('\n').to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
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
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        messages: &[DisplayMessage],
        show_thinking: bool,
        focused: bool,
        blink_on: bool,
        selected_tool_block: Option<usize>,
        selection: Option<&Selection>,
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

            // Tool result selection style
            let tool_select_style = if msg.role == MessageRole::ToolResult {
                let mut style = Style::default().fg(role_color);
                if selected_tool_block == Some(msg_idx) {
                    style = style.add_modifier(Modifier::BOLD);
                }
                Some(style)
            } else {
                None
            };

            // Collapsed tool result: show one-line summary
            if msg.role == MessageRole::ToolResult && msg.collapsed {
                all_lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(role_color)),
                    Span::styled("▶ ", tool_select_style.unwrap_or_default()),
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
            let indicator = tool_select_style.map(|style| Span::styled("▼ ", style));

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

            // Thinking section (dimmed, not collapsible)
            if show_thinking
                && let Some(thinking) = &msg.thinking
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

            // Streaming cursor — always occupies a line so rendered_lines stays
            // stable across blink cycles (prevents scroll jitter).
            if msg.is_streaming {
                let cursor_char = if blink_on { "█" } else { " " };
                all_lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(role_color)),
                    Span::styled(cursor_char, Style::default().fg(role_color)),
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

        // Capture the inner-area cells so selection copy can extract exactly
        // what the user sees (after wrapping). Then apply the selection
        // highlight on top.
        let inner_x = area.x.saturating_add(1);
        let inner_y = area.y.saturating_add(1);
        let inner_w = area.width.saturating_sub(2);
        let inner_h = area.height.saturating_sub(2);
        let buf = frame.buffer_mut();

        let mut rows: Vec<Vec<String>> = Vec::with_capacity(inner_h as usize);
        for y in 0..inner_h {
            let mut row: Vec<String> = Vec::with_capacity(inner_w as usize);
            for x in 0..inner_w {
                let symbol = buf
                    .cell((inner_x + x, inner_y + y))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_default();
                row.push(symbol);
            }
            rows.push(row);
        }
        self.visible_cells = rows;

        if let Some(sel) = selection
            && !sel.is_empty()
        {
            let (start, end) = sel.normalized();
            let highlight = Style::default().add_modifier(Modifier::REVERSED);
            for row_idx in start.0..=end.0 {
                if row_idx >= inner_h {
                    break;
                }
                let col_start = if row_idx == start.0 { start.1 } else { 0 };
                let col_end = if row_idx == end.0 { end.1 } else { inner_w };
                let col_start = col_start.min(inner_w);
                let col_end = col_end.min(inner_w).max(col_start);
                for col in col_start..col_end {
                    if let Some(cell) = buf.cell_mut((inner_x + col, inner_y + row_idx)) {
                        cell.set_style(highlight);
                    }
                }
            }
        }
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

    #[test]
    fn auto_scroll_disengages_on_manual_scroll_up() {
        let mut view = ConversationView::new();
        assert!(view.auto_scroll, "auto_scroll should start true");

        view.scroll_offset = 10;
        view.scroll_up(3);

        assert_eq!(view.scroll_offset, 7);
        assert!(
            !view.auto_scroll,
            "auto_scroll should disengage on manual scroll up"
        );
    }

    #[test]
    fn auto_scroll_reengages_at_bottom() {
        let mut view = ConversationView::new();
        view.set_rendered_lines_for_test(50);
        view.auto_scroll = false;
        view.scroll_offset = 35;

        // Scroll down enough to reach the bottom (max = 50 - 10 = 40)
        view.scroll_down(10, 10);

        assert_eq!(view.scroll_offset, 40);
        assert!(
            view.auto_scroll,
            "auto_scroll should re-engage when scrolled to bottom"
        );
    }

    #[test]
    fn clamp_scroll_prevents_negative() {
        let mut view = ConversationView::new();
        view.scroll_offset = 2;

        // Scroll up more than the current offset
        view.scroll_up(10);

        assert_eq!(view.scroll_offset, 0, "scroll offset should clamp at 0");
        assert!(!view.auto_scroll);
    }

    #[test]
    fn scroll_down_past_content_clamps() {
        let mut view = ConversationView::new();
        view.set_rendered_lines_for_test(20);
        view.auto_scroll = false;
        view.scroll_offset = 5;

        // visible_height = 10, max = 20 - 10 = 10
        // scroll_offset = 5 + 100 = 105, clamped to 10
        view.scroll_down(100, 10);

        assert_eq!(view.scroll_offset, 10, "scroll offset should clamp to max");
        assert!(view.auto_scroll, "auto_scroll should re-engage at bottom");
    }

    #[test]
    fn scroll_to_bottom_sets_max_and_reengages() {
        let mut view = ConversationView::new();
        view.set_rendered_lines_for_test(30);
        view.auto_scroll = false;
        view.scroll_offset = 0;

        view.scroll_to_bottom(10);

        assert_eq!(view.scroll_offset, 20);
        assert!(view.auto_scroll);
    }

    #[test]
    fn new_view_starts_with_auto_scroll_at_zero() {
        let view = ConversationView::new();
        assert_eq!(view.scroll_offset, 0);
        assert!(view.auto_scroll);
    }

    #[test]
    fn clamp_noop_when_within_bounds() {
        let mut view = ConversationView::new();
        view.set_rendered_lines_for_test(30);
        view.scroll_offset = 5;

        view.clamp_scroll_offset(10);

        // max = 30 - 10 = 20, offset 5 is within bounds
        assert_eq!(
            view.scroll_offset, 5,
            "should not change when within bounds"
        );
    }

    #[test]
    fn scroll_down_not_at_bottom_does_not_reengage() {
        let mut view = ConversationView::new();
        view.set_rendered_lines_for_test(50);
        view.auto_scroll = false;
        view.scroll_offset = 0;

        // max = 50 - 10 = 40, scroll to 5 which is not at bottom
        view.scroll_down(5, 10);

        assert_eq!(view.scroll_offset, 5);
        assert!(!view.auto_scroll, "should not re-engage when not at bottom");
    }
}
