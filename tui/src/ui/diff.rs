//! Inline diff rendering for file modifications.
//!
//! Computes and renders unified diffs from old/new file content provided
//! by `WriteFileTool`'s `details` field.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme;

/// Maximum number of diff output lines before truncation.
const MAX_DIFF_LINES: usize = 50;

/// A parsed diff from tool result details.
#[derive(Debug, Clone)]
pub struct DiffData {
    /// File path that was modified.
    pub path: String,
    /// Whether this was a newly created file.
    pub is_new_file: bool,
    /// Content before the write (empty for new files).
    pub old_content: String,
    /// Content after the write.
    pub new_content: String,
}

impl DiffData {
    /// Try to parse diff data from a tool result's details JSON.
    ///
    /// Returns `None` if the JSON does not contain the expected fields.
    pub fn from_details(details: &serde_json::Value) -> Option<Self> {
        let path = details.get("path")?.as_str()?.to_string();
        let is_new_file = details.get("is_new_file")?.as_bool()?;
        let old_content = details.get("old_content")?.as_str()?.to_string();
        let new_content = details.get("new_content")?.as_str()?.to_string();
        Some(Self {
            path,
            is_new_file,
            old_content,
            new_content,
        })
    }
}

/// Render a unified diff as styled terminal lines.
pub fn render_diff_lines(diff: &DiffData, max_width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let width = max_width as usize;

    // Header
    let header_style = Style::default()
        .fg(theme::border_focused_color())
        .add_modifier(Modifier::BOLD);
    if diff.is_new_file {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(format!("+ new file: {}", diff.path), header_style),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(format!("--- {}", diff.path), header_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(format!("+++ {}", diff.path), header_style),
        ]));
    }

    let old_lines: Vec<&str> = diff.old_content.lines().collect();
    let new_lines: Vec<&str> = diff.new_content.lines().collect();

    if diff.is_new_file {
        // All additions
        for line in &new_lines {
            let display = truncate_line(line, width.saturating_sub(4));
            lines.push(Line::from(vec![Span::styled(
                format!("  + {display}"),
                Style::default().fg(theme::diff_add_color()),
            )]));
        }
        return lines;
    }

    // Compute LCS-based diff
    let lcs = compute_lcs(&old_lines, &new_lines);
    let mut old_idx = 0;
    let mut new_idx = 0;
    let mut lcs_idx = 0;

    while old_idx < old_lines.len() || new_idx < new_lines.len() {
        if lcs_idx < lcs.len() && old_idx == lcs[lcs_idx].0 && new_idx == lcs[lcs_idx].1 {
            // Context line (common)
            let display = truncate_line(old_lines[old_idx], width.saturating_sub(4));
            lines.push(Line::from(vec![Span::styled(
                format!("    {display}"),
                Style::default().add_modifier(Modifier::DIM),
            )]));
            old_idx += 1;
            new_idx += 1;
            lcs_idx += 1;
        } else {
            // Removed lines
            while old_idx < old_lines.len() && (lcs_idx >= lcs.len() || old_idx < lcs[lcs_idx].0) {
                let display = truncate_line(old_lines[old_idx], width.saturating_sub(4));
                lines.push(Line::from(vec![Span::styled(
                    format!("  - {display}"),
                    Style::default().fg(theme::diff_remove_color()),
                )]));
                old_idx += 1;
            }
            // Added lines
            while new_idx < new_lines.len() && (lcs_idx >= lcs.len() || new_idx < lcs[lcs_idx].1) {
                let display = truncate_line(new_lines[new_idx], width.saturating_sub(4));
                lines.push(Line::from(vec![Span::styled(
                    format!("  + {display}"),
                    Style::default().fg(theme::diff_add_color()),
                )]));
                new_idx += 1;
            }
        }
    }

    // Limit total diff output to avoid overwhelming the conversation
    if lines.len() > MAX_DIFF_LINES {
        let truncated = lines.len() - MAX_DIFF_LINES;
        lines.truncate(MAX_DIFF_LINES);
        lines.push(Line::from(vec![Span::styled(
            format!("  ... ({truncated} more lines)"),
            Style::default().add_modifier(Modifier::DIM),
        )]));
    }

    lines
}

/// Truncate a line to max characters.
fn truncate_line(line: &str, max: usize) -> String {
    if line.len() <= max {
        line.to_string()
    } else {
        format!("{}...", &line[..max.saturating_sub(3)])
    }
}

/// Compute the longest common subsequence of two line slices.
///
/// Returns a vector of `(old_index, new_index)` pairs identifying matching lines.
fn compute_lcs(old: &[&str], new: &[&str]) -> Vec<(usize, usize)> {
    let m = old.len();
    let n = new.len();
    if m == 0 || n == 0 {
        return Vec::new();
    }

    // DP table
    let mut dp = vec![vec![0u32; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            dp[i][j] = if old[i] == new[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    // Backtrace
    let mut result = Vec::new();
    let mut i = 0;
    let mut j = 0;
    while i < m && j < n {
        if old[i] == new[j] {
            result.push((i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn diff_data_from_valid_details() {
        let details = serde_json::json!({
            "path": "/tmp/test.rs",
            "is_new_file": false,
            "old_content": "hello\nworld",
            "new_content": "hello\nrust",
            "bytes_written": 10,
        });
        let diff = DiffData::from_details(&details).unwrap();
        assert_eq!(diff.path, "/tmp/test.rs");
        assert!(!diff.is_new_file);
        assert_eq!(diff.old_content, "hello\nworld");
        assert_eq!(diff.new_content, "hello\nrust");
    }

    #[test]
    fn diff_data_from_null_returns_none() {
        assert!(DiffData::from_details(&serde_json::Value::Null).is_none());
    }

    #[test]
    fn diff_data_from_missing_field_returns_none() {
        let details = serde_json::json!({"path": "/tmp/test.rs"});
        assert!(DiffData::from_details(&details).is_none());
    }

    #[test]
    fn render_new_file_shows_all_additions() {
        let diff = DiffData {
            path: "/tmp/test.rs".to_string(),
            is_new_file: true,
            old_content: String::new(),
            new_content: "line1\nline2\nline3".to_string(),
        };
        let lines = render_diff_lines(&diff, 80);
        // Header (1 line) + 3 added lines
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn render_modification_shows_removals_and_additions() {
        let diff = DiffData {
            path: "/tmp/test.rs".to_string(),
            is_new_file: false,
            old_content: "line1\nold\nline3".to_string(),
            new_content: "line1\nnew\nline3".to_string(),
        };
        let lines = render_diff_lines(&diff, 80);
        // Header (2 lines) + line1 (context) + old (removed) + new (added) + line3 (context)
        assert!(lines.len() >= 5);
        // Check that we have both red and green lines
        let has_removed = lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.style.fg == Some(Color::Red)));
        let has_added = lines
            .iter()
            .any(|l| l.spans.iter().any(|s| s.style.fg == Some(Color::Green)));
        assert!(has_removed, "should have removed lines (red)");
        assert!(has_added, "should have added lines (green)");
    }

    #[test]
    fn render_identical_content_shows_only_context() {
        let diff = DiffData {
            path: "/tmp/test.rs".to_string(),
            is_new_file: false,
            old_content: "line1\nline2".to_string(),
            new_content: "line1\nline2".to_string(),
        };
        let lines = render_diff_lines(&diff, 80);
        // Header + 2 context lines (all dim, no red/green)
        let has_changes = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.style.fg == Some(Color::Red) || s.style.fg == Some(Color::Green))
        });
        assert!(!has_changes, "identical content should show no red/green");
    }

    #[test]
    fn compute_lcs_empty() {
        assert!(compute_lcs(&[], &[]).is_empty());
        assert!(compute_lcs(&["a"], &[]).is_empty());
        assert!(compute_lcs(&[], &["a"]).is_empty());
    }

    #[test]
    fn compute_lcs_identical() {
        let result = compute_lcs(&["a", "b", "c"], &["a", "b", "c"]);
        assert_eq!(result, vec![(0, 0), (1, 1), (2, 2)]);
    }

    #[test]
    fn compute_lcs_partial_match() {
        let result = compute_lcs(&["a", "b", "c"], &["a", "x", "c"]);
        assert_eq!(result, vec![(0, 0), (2, 2)]);
    }

    #[test]
    fn truncate_line_short() {
        assert_eq!(truncate_line("hello", 10), "hello");
    }

    #[test]
    fn truncate_line_long() {
        let long = "a".repeat(100);
        let result = truncate_line(&long, 20);
        assert!(result.len() <= 20);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn large_diff_is_truncated() {
        let old = (0..100)
            .map(|i| format!("old line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let new = (0..100)
            .map(|i| format!("new line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let diff = DiffData {
            path: "/tmp/test.rs".to_string(),
            is_new_file: false,
            old_content: old,
            new_content: new,
        };
        let lines = render_diff_lines(&diff, 80);
        // Should be truncated to ~51 lines (50 + truncation notice)
        assert!(
            lines.len() <= 53,
            "diff should be truncated, got {} lines",
            lines.len()
        );
    }
}
