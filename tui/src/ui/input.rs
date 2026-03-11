//! Multi-line input editor widget.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

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
        if self.cursor_col > self.lines[self.cursor_row].len() {
            self.cursor_col = self.lines[self.cursor_row].len();
        }
        self.lines[self.cursor_row].insert(self.cursor_col, c);
        self.cursor_col += 1;
    }

    /// Insert a newline at the cursor position (Shift+Enter).
    pub fn insert_newline(&mut self) {
        let current = &self.lines[self.cursor_row];
        let remainder = current[self.cursor_col..].to_string();
        self.lines[self.cursor_row].truncate(self.cursor_col);
        self.cursor_row += 1;
        self.lines.insert(self.cursor_row, remainder);
        self.cursor_col = 0;
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            self.lines[self.cursor_row].remove(self.cursor_col);
        } else if self.cursor_row > 0 {
            // Merge with previous line
            let current = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
            self.lines[self.cursor_row].push_str(&current);
        }
    }

    /// Delete the character at the cursor.
    pub fn delete(&mut self) {
        let line_len = self.lines[self.cursor_row].len();
        if self.cursor_col < line_len {
            self.lines[self.cursor_row].remove(self.cursor_col);
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
            self.cursor_col = self.lines[self.cursor_row].len();
        }
    }

    /// Move cursor right.
    pub fn move_right(&mut self) {
        let line_len = self.lines[self.cursor_row].len();
        if self.cursor_col < line_len {
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
            self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
        }
    }

    /// Move cursor down.
    pub fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
        }
    }

    /// Move cursor to start of line (Home / Ctrl+A).
    pub const fn move_home(&mut self) {
        self.cursor_col = 0;
    }

    /// Move cursor to end of line (End / Ctrl+E).
    pub fn move_end(&mut self) {
        self.cursor_col = self.lines[self.cursor_row].len();
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
        self.cursor_col = self.lines[self.cursor_row].len();
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
            self.cursor_col = self.lines[self.cursor_row].len();
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
            Color::Cyan
        } else {
            Color::DarkGray
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
                                .fg(Color::DarkGray)
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
