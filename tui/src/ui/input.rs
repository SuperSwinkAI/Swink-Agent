//! Multi-line input editor widget.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::theme;

/// Multi-line input editor state.
pub struct InputEditor {
    /// Lines of text in the editor.
    pub lines: Vec<String>,
    /// Current cursor row (0-indexed).
    pub cursor_row: usize,
    /// Current cursor column (0-indexed).
    pub cursor_col: usize,
    /// Scroll offset for when content exceeds visible area.
    pub scroll_offset: usize,
    /// Input history for Up/Down recall.
    pub history: Vec<Vec<String>>,
    /// Current index into history (None = editing new input).
    pub history_index: Option<usize>,
    /// Saved in-progress input when browsing history.
    saved_input: Option<Vec<String>>,
}

impl Default for InputEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl InputEditor {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            scroll_offset: 0,
            history: Vec::new(),
            history_index: None,
            saved_input: None,
        }
    }

    /// Char count of the current line.
    fn line_char_len(&self) -> usize {
        self.lines[self.cursor_row].chars().count()
    }

    /// Convert a char index to a byte offset in the given line.
    fn char_to_byte(line: &str, char_idx: usize) -> usize {
        line.char_indices()
            .nth(char_idx)
            .map_or(line.len(), |(byte_idx, _)| byte_idx)
    }

    /// Convert a byte offset to a char index.
    fn byte_to_char(line: &str) -> usize {
        line.chars().count()
    }

    /// Get the dynamic height for the input area.
    /// Grows with content from 3 to a max of 10 lines.
    pub fn height(&self) -> u16 {
        let content_height = self.lines.len() + 2; // +2 for borders
        #[allow(clippy::cast_possible_truncation)]
        {
            content_height.clamp(3, 10) as u16
        }
    }

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, c: char) {
        let char_count = self.line_char_len();
        if self.cursor_col > char_count {
            self.cursor_col = char_count;
        }
        let byte_idx = Self::char_to_byte(&self.lines[self.cursor_row], self.cursor_col);
        self.lines[self.cursor_row].insert(byte_idx, c);
        self.cursor_col += 1;
    }

    /// Insert a newline at the cursor position (Shift+Enter).
    pub fn insert_newline(&mut self) {
        let byte_idx = Self::char_to_byte(&self.lines[self.cursor_row], self.cursor_col);
        let remainder = self.lines[self.cursor_row][byte_idx..].to_string();
        self.lines[self.cursor_row].truncate(byte_idx);
        self.cursor_row += 1;
        self.lines.insert(self.cursor_row, remainder);
        self.cursor_col = 0;
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            let byte_idx = Self::char_to_byte(&self.lines[self.cursor_row], self.cursor_col);
            // Find the byte range of the char at this position
            let ch = self.lines[self.cursor_row][byte_idx..]
                .chars()
                .next()
                .unwrap();
            self.lines[self.cursor_row].replace_range(byte_idx..byte_idx + ch.len_utf8(), "");
        } else if self.cursor_row > 0 {
            // Merge with previous line
            let current = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = Self::byte_to_char(&self.lines[self.cursor_row]);
            self.lines[self.cursor_row].push_str(&current);
        }
    }

    /// Delete the character at the cursor.
    pub fn delete(&mut self) {
        let char_count = self.line_char_len();
        if self.cursor_col < char_count {
            let byte_idx = Self::char_to_byte(&self.lines[self.cursor_row], self.cursor_col);
            let ch = self.lines[self.cursor_row][byte_idx..]
                .chars()
                .next()
                .unwrap();
            self.lines[self.cursor_row].replace_range(byte_idx..byte_idx + ch.len_utf8(), "");
        } else if self.cursor_row + 1 < self.lines.len() {
            // Merge with next line
            let next = self.lines.remove(self.cursor_row + 1);
            self.lines[self.cursor_row].push_str(&next);
        }
    }

    /// Move cursor left.
    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = Self::byte_to_char(&self.lines[self.cursor_row]);
        }
    }

    /// Move cursor right.
    pub fn move_right(&mut self) {
        let char_count = self.line_char_len();
        if self.cursor_col < char_count {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    /// Move cursor up.
    pub fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            let char_count = Self::byte_to_char(&self.lines[self.cursor_row]);
            self.cursor_col = self.cursor_col.min(char_count);
        }
    }

    /// Move cursor down.
    pub fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            let char_count = Self::byte_to_char(&self.lines[self.cursor_row]);
            self.cursor_col = self.cursor_col.min(char_count);
        }
    }

    /// Move cursor to start of line (Home / Ctrl+A).
    pub const fn move_home(&mut self) {
        self.cursor_col = 0;
    }

    /// Move cursor to end of line (End / Ctrl+E).
    pub fn move_end(&mut self) {
        self.cursor_col = Self::byte_to_char(&self.lines[self.cursor_row]);
    }

    /// Submit the current input, returning the full text. Clears the editor.
    /// Returns None if the input is empty/whitespace.
    pub fn submit(&mut self) -> Option<String> {
        let text: String = self.lines.join("\n");
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return None;
        }
        // Save to history
        self.history.push(self.lines.clone());
        // Reset editor
        self.lines = vec![String::new()];
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.scroll_offset = 0;
        self.history_index = None;
        self.saved_input = None;
        Some(trimmed)
    }

    /// Navigate to previous history entry.
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                // Save current input and go to most recent history
                self.saved_input = Some(self.lines.clone());
                let idx = self.history.len() - 1;
                self.history_index = Some(idx);
                self.lines = self.history[idx].clone();
            }
            Some(idx) if idx > 0 => {
                let new_idx = idx - 1;
                self.history_index = Some(new_idx);
                self.lines = self.history[new_idx].clone();
            }
            _ => return,
        }
        self.cursor_row = self.lines.len().saturating_sub(1);
        self.cursor_col = Self::byte_to_char(&self.lines[self.cursor_row]);
    }

    /// Navigate to next history entry.
    pub fn history_next(&mut self) {
        if let Some(idx) = self.history_index {
            if idx + 1 < self.history.len() {
                let new_idx = idx + 1;
                self.history_index = Some(new_idx);
                self.lines = self.history[new_idx].clone();
            } else {
                // Restore saved input
                self.history_index = None;
                if let Some(saved) = self.saved_input.take() {
                    self.lines = saved;
                } else {
                    self.lines = vec![String::new()];
                }
            }
            self.cursor_row = self.lines.len().saturating_sub(1);
            self.cursor_col = Self::byte_to_char(&self.lines[self.cursor_row]);
        }
    }

    /// Check if content is empty.
    ///
    /// Reserved for future use by input validation logic.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(String::is_empty)
    }

    /// Check if this is a multi-line input.
    pub const fn is_multiline(&self) -> bool {
        self.lines.len() > 1
    }

    /// Render the input editor.
    pub fn render(&self, frame: &mut Frame, area: Rect, focused: bool, status_hint: &str) {
        let border_color = if focused {
            theme::assistant_color()
        } else {
            theme::border_color()
        };

        let title = if status_hint.is_empty() {
            " Message ".to_string()
        } else {
            format!(" Message ({status_hint}) ")
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(border_color));

        // Build text lines with optional line numbers for multi-line
        let text_lines: Vec<Line> = if self.is_multiline() {
            self.lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    Line::from(vec![
                        Span::styled(
                            format!("{:>2} ", i + 1),
                            Style::default()
                                .fg(theme::border_color())
                                .add_modifier(Modifier::DIM),
                        ),
                        Span::raw(line.clone()),
                    ])
                })
                .collect()
        } else {
            self.lines.iter().map(|l| Line::from(l.clone())).collect()
        };

        let paragraph = Paragraph::new(text_lines)
            .block(block)
            .wrap(Wrap { trim: false });

        frame.render_widget(paragraph, area);

        // Position cursor
        if focused {
            #[allow(clippy::cast_possible_truncation)]
            let gutter_width: u16 = if self.is_multiline() { 3 } else { 0 };
            let visible_row = self.cursor_row.saturating_sub(self.scroll_offset);
            #[allow(clippy::cast_possible_truncation)]
            let cursor_x = area.x + 1 + gutter_width + self.cursor_col as u16;
            #[allow(clippy::cast_possible_truncation)]
            let cursor_y = area.y + 1 + visible_row as u16;
            if cursor_y < area.y + area.height - 1 {
                frame.set_cursor_position((cursor_x.min(area.x + area.width - 2), cursor_y));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_editor_is_empty() {
        let editor = InputEditor::new();
        assert_eq!(editor.lines, vec![String::new()]);
        assert_eq!(editor.cursor_row, 0);
        assert_eq!(editor.cursor_col, 0);
        assert!(editor.is_empty());
    }

    #[test]
    fn insert_char_at_start() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        assert_eq!(editor.lines, vec!["a".to_string()]);
        assert_eq!(editor.cursor_col, 1);
    }

    #[test]
    fn insert_char_at_end() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_char('b');
        assert_eq!(editor.lines, vec!["ab".to_string()]);
        assert_eq!(editor.cursor_col, 2);
    }

    #[test]
    fn backspace_at_start_does_nothing() {
        let mut editor = InputEditor::new();
        editor.backspace();
        assert_eq!(editor.lines, vec![String::new()]);
        assert_eq!(editor.cursor_row, 0);
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn backspace_merges_lines() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        assert_eq!(editor.lines.len(), 2);
        // Move cursor to start of line 2
        editor.move_home();
        editor.backspace();
        assert_eq!(editor.lines, vec!["ab".to_string()]);
        assert_eq!(editor.cursor_row, 0);
        assert_eq!(editor.cursor_col, 1);
    }

    #[test]
    fn delete_at_end_does_nothing() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.delete();
        assert_eq!(editor.lines, vec!["a".to_string()]);
    }

    #[test]
    fn delete_merges_with_next_line() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        // Move cursor to end of first line
        editor.cursor_row = 0;
        editor.cursor_col = 1;
        editor.delete();
        assert_eq!(editor.lines, vec!["ab".to_string()]);
    }

    #[test]
    fn insert_newline_splits_line() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_char('b');
        // Move cursor between a and b
        editor.cursor_col = 1;
        editor.insert_newline();
        assert_eq!(editor.lines, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(editor.cursor_row, 1);
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn move_left_at_start_wraps_to_previous_line() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        // Cursor is at (1, 0)
        editor.move_left();
        assert_eq!(editor.cursor_row, 0);
        assert_eq!(editor.cursor_col, 1); // end of "a"
    }

    #[test]
    fn move_right_at_end_wraps_to_next_line() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.insert_newline();
        editor.insert_char('b');
        // Move to end of first line
        editor.cursor_row = 0;
        editor.cursor_col = 1;
        editor.move_right();
        assert_eq!(editor.cursor_row, 1);
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn move_up_at_top_stays() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.move_up();
        assert_eq!(editor.cursor_row, 0);
    }

    #[test]
    fn move_down_at_bottom_stays() {
        let mut editor = InputEditor::new();
        editor.insert_char('a');
        editor.move_down();
        assert_eq!(editor.cursor_row, 0);
    }

    #[test]
    fn history_prev_and_next() {
        let mut editor = InputEditor::new();
        // Submit "first"
        editor.insert_char('f');
        editor.insert_char('i');
        editor.insert_char('r');
        editor.insert_char('s');
        editor.insert_char('t');
        editor.submit();
        // Submit "second"
        editor.insert_char('s');
        editor.insert_char('e');
        editor.insert_char('c');
        editor.insert_char('o');
        editor.insert_char('n');
        editor.insert_char('d');
        editor.submit();
        // Navigate backwards
        editor.history_prev();
        assert_eq!(editor.lines, vec!["second".to_string()]);
        editor.history_prev();
        assert_eq!(editor.lines, vec!["first".to_string()]);
        // Navigate forward
        editor.history_next();
        assert_eq!(editor.lines, vec!["second".to_string()]);
        editor.history_next();
        // Should restore to empty (saved input)
        assert_eq!(editor.lines, vec![String::new()]);
    }

    #[test]
    fn submit_clears_and_returns_text() {
        let mut editor = InputEditor::new();
        editor.insert_char('h');
        editor.insert_char('i');
        let result = editor.submit();
        assert_eq!(result, Some("hi".to_string()));
        assert_eq!(editor.lines, vec![String::new()]);
        assert_eq!(editor.cursor_row, 0);
        assert_eq!(editor.cursor_col, 0);
        assert_eq!(editor.history.len(), 1);
    }

    #[test]
    fn height_clamps_between_min_max() {
        let editor = InputEditor::new();
        // 1 line + 2 borders = 3, clamped to min 3
        assert_eq!(editor.height(), 3);

        let mut editor = InputEditor::new();
        // Add 20 lines to exceed max
        for _ in 0..20 {
            editor.insert_newline();
        }
        assert_eq!(editor.height(), 10);
    }

    #[test]
    fn insert_emoji_and_cursor_advances() {
        let mut editor = InputEditor::new();
        editor.insert_char('🎉');
        assert_eq!(editor.lines[0], "🎉");
        assert_eq!(editor.cursor_col, 1);
        editor.insert_char('x');
        assert_eq!(editor.lines[0], "🎉x");
        assert_eq!(editor.cursor_col, 2);
    }

    #[test]
    fn insert_cjk_characters() {
        let mut editor = InputEditor::new();
        editor.insert_char('你');
        editor.insert_char('好');
        assert_eq!(editor.lines[0], "你好");
        assert_eq!(editor.cursor_col, 2);
        editor.move_left();
        assert_eq!(editor.cursor_col, 1);
        editor.backspace();
        assert_eq!(editor.lines[0], "好");
        assert_eq!(editor.cursor_col, 0);
    }

    #[test]
    fn insert_combining_characters() {
        let mut editor = InputEditor::new();
        // e followed by combining acute accent (two chars, one grapheme)
        editor.insert_char('e');
        editor.insert_char('\u{0301}');
        assert_eq!(editor.lines[0], "e\u{0301}");
        assert_eq!(editor.cursor_col, 2);
    }

    #[test]
    fn large_paste_does_not_panic() {
        let mut editor = InputEditor::new();
        let large_text: String = "a".repeat(10_000);
        for c in large_text.chars() {
            editor.insert_char(c);
        }
        assert_eq!(editor.lines[0].len(), 10_000);
        assert_eq!(editor.cursor_col, 10_000);
        // Verify submit works with large content
        let result = editor.submit();
        assert!(result.is_some());
        assert_eq!(result.unwrap().len(), 10_000);
    }
}
