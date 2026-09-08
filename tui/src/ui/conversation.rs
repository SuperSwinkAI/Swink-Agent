//! Scrollable conversation view with role-colored message blocks.

use std::hash::{DefaultHasher, Hash, Hasher};

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
    /// Whether hidden channels (currently: reasoning/thinking content) render
    /// inline for every message instead of the collapsed `[thinking...]`
    /// placeholder. Session-stateful; toggled globally via F5. Off by default.
    pub show_hidden_channels: bool,
    /// Total rendered lines (computed each frame).
    rendered_lines: usize,
    /// Per-row cell symbols captured from the last render pass while a
    /// selection was active, indexed `[row][col]` over the conversation's
    /// inner area. Used by selection copy to extract exactly what the user
    /// sees on screen. Empty when no selection is active.
    pub(crate) visible_cells: Vec<Vec<String>>,
    /// Per-message render cache (see `RenderCache`).
    cache: RenderCache,
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
            show_hidden_channels: false,
            rendered_lines: 0,
            visible_cells: Vec::new(),
            cache: RenderCache::new(),
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
    ///
    /// Inline visibility of hidden channels is controlled by the
    /// [`show_hidden_channels`](Self::show_hidden_channels) field rather
    /// than a parameter.
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

        let ctx = RenderContext {
            show_thinking,
            show_hidden_channels: self.show_hidden_channels,
            inner_width,
            blink_on,
        };

        // Invalidate the cache when a global render input changes, then
        // rebuild per-message lines only where the message itself changed.
        self.cache.sync(
            inner_width,
            theme::color_mode(),
            show_thinking,
            self.show_hidden_channels,
            messages.len(),
        );

        // Build all lines from messages, reusing cached lines where possible.
        let mut all_lines: Vec<Line<'static>> = Vec::new();

        for (msg_idx, msg) in messages.iter().enumerate() {
            let selected = selected_tool_block == Some(msg_idx);

            // The streaming message changes with every token (and its cursor
            // blinks), so render it fresh and drop any stale cache slot.
            if msg.is_streaming {
                self.cache.entries[msg_idx] = None;
                all_lines.extend(render_message_lines(msg, selected, ctx));
                continue;
            }

            let fingerprint = message_fingerprint(msg, selected);
            let cache_hit = matches!(
                &self.cache.entries[msg_idx],
                Some(cached) if cached.fingerprint == fingerprint
            );
            if !cache_hit {
                let lines = render_message_lines(msg, selected, ctx);
                self.cache.entries[msg_idx] = Some(CachedMessage { fingerprint, lines });
            }
            if let Some(cached) = &self.cache.entries[msg_idx] {
                all_lines.extend(cached.lines.iter().cloned());
            }
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

        // With an active selection, capture the inner-area cells so selection
        // copy can extract exactly what the user sees (after wrapping), then
        // apply the selection highlight on top. Without one, skip the capture
        // entirely — it allocates `inner_w * inner_h` Strings per frame.
        if let Some(sel) = selection {
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

            if !sel.is_empty() {
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
        } else if !self.visible_cells.is_empty() {
            // No active selection: drop the capture so copy cannot read
            // cells from a stale frame.
            self.visible_cells = Vec::new();
        }
    }
}

/// Rendered lines for a single message plus the fingerprint of the inputs
/// that produced them.
struct CachedMessage {
    /// Hash of every message-local input that affects rendered output.
    fingerprint: u64,
    /// Fully rendered lines for the message, including the gutter, role
    /// header, content, diff, and trailing blank separator line.
    lines: Vec<Line<'static>>,
}

/// Per-message render cache.
///
/// Rebuilding every message on each dirty frame re-runs the markdown parser
/// and syntect highlighting for the whole conversation on every streamed
/// token. Instead, rendered lines are cached per message: global inputs
/// (inner width, color mode, thinking visibility, hidden-channels toggle)
/// clear the whole cache when they change, and each entry is invalidated
/// individually via a fingerprint
/// of the message fields that affect its output. The actively streaming
/// message is never cached.
struct RenderCache {
    /// Inner width the entries were rendered at.
    inner_width: u16,
    /// Theme color mode the entries were rendered with.
    color_mode: theme::ColorMode,
    /// Whether thinking sections were visible when entries were rendered.
    show_thinking: bool,
    /// Whether hidden channels rendered inline (vs. collapsed placeholder)
    /// when entries were rendered.
    show_hidden_channels: bool,
    /// One slot per message, parallel to the message list.
    entries: Vec<Option<CachedMessage>>,
}

impl RenderCache {
    const fn new() -> Self {
        Self {
            inner_width: 0,
            color_mode: theme::ColorMode::Custom,
            show_thinking: false,
            show_hidden_channels: false,
            entries: Vec::new(),
        }
    }

    /// Drop every entry when a global render input changes, then resize the
    /// entry table to `len` messages (new slots start empty; shrinking
    /// truncates).
    fn sync(
        &mut self,
        inner_width: u16,
        color_mode: theme::ColorMode,
        show_thinking: bool,
        show_hidden_channels: bool,
        len: usize,
    ) {
        if self.inner_width != inner_width
            || self.color_mode != color_mode
            || self.show_thinking != show_thinking
            || self.show_hidden_channels != show_hidden_channels
        {
            self.entries.clear();
            self.inner_width = inner_width;
            self.color_mode = color_mode;
            self.show_thinking = show_thinking;
            self.show_hidden_channels = show_hidden_channels;
        }
        self.entries.resize_with(len, || None);
    }
}

/// Fingerprint of every message-local input that affects rendered output.
///
/// Global inputs (width, color mode, thinking visibility, hidden-channels
/// toggle) are handled by [`RenderCache::sync`] and deliberately excluded
/// here. `blink_on` only
/// affects streaming messages, which are never cached.
fn message_fingerprint(msg: &DisplayMessage, selected: bool) -> u64 {
    let mut hasher = DefaultHasher::new();
    // `DisplayRole` is #[non_exhaustive] (no discriminant cast outside its
    // crate), so tag roles the same way rendering groups them: `System` and
    // any unknown future role render identically.
    let role_tag: u8 = match msg.role {
        MessageRole::User => 0,
        MessageRole::Assistant => 1,
        MessageRole::ToolResult => 2,
        MessageRole::Error => 3,
        _ => 4,
    };
    role_tag.hash(&mut hasher);
    msg.plan_mode.hash(&mut hasher);
    msg.collapsed.hash(&mut hasher);
    msg.summary.hash(&mut hasher);
    msg.content.hash(&mut hasher);
    // Only the presence of non-empty thinking affects output.
    let has_thinking = msg.thinking.as_ref().is_some_and(|t| !t.is_empty());
    has_thinking.hash(&mut hasher);
    msg.diff_data.is_some().hash(&mut hasher);
    if let Some(diff) = &msg.diff_data {
        diff.path.hash(&mut hasher);
        diff.is_new_file.hash(&mut hasher);
        diff.old_content.hash(&mut hasher);
        diff.new_content.hash(&mut hasher);
    }
    selected.hash(&mut hasher);
    hasher.finish()
}

/// Global render inputs shared by every message in a frame.
///
/// Groups the per-frame globals so per-message rendering takes one context
/// instead of a widening list of loose parameters.
#[derive(Clone, Copy)]
struct RenderContext {
    /// Whether thinking sections are shown at all (config-driven).
    show_thinking: bool,
    /// Whether hidden channels render inline instead of the collapsed
    /// `[thinking...]` placeholder (F5 toggle).
    show_hidden_channels: bool,
    /// Inner width of the conversation viewport, for word-wrapping.
    inner_width: u16,
    /// Blink phase for the streaming cursor.
    blink_on: bool,
}

/// Render one message into its display lines, including the colored gutter,
/// role header, optional thinking section, markdown content, optional diff,
/// optional streaming cursor, and the trailing blank separator line.
///
/// When `ctx.show_hidden_channels` is on, the thinking section renders the
/// full reasoning text inline (dim + italic, so it never reads as the
/// assistant's visible reply); otherwise it collapses to the `[thinking...]`
/// placeholder.
#[allow(clippy::too_many_lines)]
fn render_message_lines(
    msg: &DisplayMessage,
    selected: bool,
    ctx: RenderContext,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();

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
        // Covers MessageRole::System and, since DisplayRole is
        // #[non_exhaustive], any unknown future role — rendered as a
        // neutral, non-attributed line.
        _ => ("System", theme::system_color()),
    };

    // Tool result selection style
    let tool_select_style = if msg.role == MessageRole::ToolResult {
        let mut style = Style::default().fg(role_color);
        if selected {
            style = style.add_modifier(Modifier::BOLD);
        }
        Some(style)
    } else {
        None
    };

    // Collapsed tool result: show one-line summary
    if msg.role == MessageRole::ToolResult && msg.collapsed {
        lines.push(Line::from(vec![
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
        lines.push(Line::from(""));
        return lines;
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
    lines.push(Line::from(header_spans));

    // Thinking section (dimmed, not collapsible)
    if ctx.show_thinking
        && let Some(thinking) = &msg.thinking
        && !thinking.is_empty()
    {
        let thinking_style = Style::default()
            .fg(theme::thinking_color())
            .add_modifier(Modifier::DIM);
        if ctx.show_hidden_channels {
            // Inline reasoning content — italic on top of the usual dim
            // thinking style so it never reads as the assistant's actual
            // visible reply, even when the model's reasoning text contains
            // markdown formatting.
            let inline_style = thinking_style.add_modifier(Modifier::ITALIC);
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(role_color)),
                Span::styled("💭 ", thinking_style),
                Span::styled("thinking:", thinking_style),
            ]));
            let thinking_lines =
                markdown::markdown_to_lines(thinking, ctx.inner_width.saturating_sub(2));
            for line in thinking_lines {
                let mut spans = vec![Span::styled("│ ", Style::default().fg(role_color))];
                spans.extend(
                    line.spans
                        .into_iter()
                        .map(|span| Span::styled(span.content, inline_style)),
                );
                lines.push(Line::from(spans));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(role_color)),
                Span::styled("💭 ", thinking_style),
                Span::styled("[thinking...]", thinking_style),
            ]));
        }
    }

    // Message content with markdown rendering
    let content_lines =
        markdown::markdown_to_lines(&msg.content, ctx.inner_width.saturating_sub(2));
    for line in content_lines {
        let mut spans = vec![Span::styled("│ ", Style::default().fg(role_color))];
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }

    // Render diff view for file modifications
    if msg.role == MessageRole::ToolResult
        && let Some(ref diff) = msg.diff_data
    {
        let diff_lines = crate::ui::diff::render_diff_lines(diff, ctx.inner_width);
        for line in diff_lines {
            let mut spans = vec![Span::styled("│ ", Style::default().fg(role_color))];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
    }

    // Streaming cursor — always occupies a line so rendered_lines stays
    // stable across blink cycles (prevents scroll jitter).
    if msg.is_streaming {
        let cursor_char = if ctx.blink_on { "█" } else { " " };
        lines.push(Line::from(vec![
            Span::styled("│ ", Style::default().fg(role_color)),
            Span::styled(cursor_char, Style::default().fg(role_color)),
        ]));
    }

    // Blank line between messages
    lines.push(Line::from(""));

    lines
}

#[cfg(test)]
#[path = "conversation_tests.rs"]
mod tests;
