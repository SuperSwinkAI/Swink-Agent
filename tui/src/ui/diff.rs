//! Inline diff rendering for file modifications.
//!
//! Computes and renders unified diffs from old/new file content provided
//! by `WriteFileTool`'s `details` field.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme;
use crate::ui::tool_panel::truncate_with_ellipsis;

/// Maximum number of diff output lines before truncation.
const MAX_DIFF_LINES: usize = 50;

/// A parsed diff from tool result details.
#[non_exhaustive]
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
    /// Create diff data from before/after content for `path`.
    ///
    /// Set `is_new_file` when there is no prior content (pass an empty
    /// `old_content` in that case).
    #[must_use]
    pub fn new(
        path: impl Into<String>,
        is_new_file: bool,
        old_content: impl Into<String>,
        new_content: impl Into<String>,
    ) -> Self {
        Self {
            path: path.into(),
            is_new_file,
            old_content: old_content.into(),
            new_content: new_content.into(),
        }
    }

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

/// A contiguous region of change between the old and new content.
///
/// Ranges are half-open line indices into the respective `lines()` split.
/// Hunks are separated by at least one unchanged (common) line, so the text
/// between two hunks is identical in both versions.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hunk {
    /// Start of the removed range in the old content (inclusive).
    pub old_start: usize,
    /// End of the removed range in the old content (exclusive).
    pub old_end: usize,
    /// Start of the added range in the new content (inclusive).
    pub new_start: usize,
    /// End of the added range in the new content (exclusive).
    pub new_end: usize,
}

impl Hunk {
    /// Create a hunk from half-open line ranges into the old and new content.
    ///
    /// `old_start..old_end` is the removed range; `new_start..new_end` is the
    /// added range.
    #[must_use]
    pub const fn new(old_start: usize, old_end: usize, new_start: usize, new_end: usize) -> Self {
        Self {
            old_start,
            old_end,
            new_start,
            new_end,
        }
    }

    /// Number of lines removed by this hunk.
    pub const fn removed_count(&self) -> usize {
        self.old_end - self.old_start
    }

    /// Number of lines added by this hunk.
    pub const fn added_count(&self) -> usize {
        self.new_end - self.new_start
    }
}

/// Split the change between `old_content` and `new_content` into hunks.
///
/// Each maximal run of non-common lines becomes one hunk. Returns an empty
/// vector when the two versions are identical.
pub fn compute_hunks(old_content: &str, new_content: &str) -> Vec<Hunk> {
    let old_lines: Vec<&str> = old_content.lines().collect();
    let new_lines: Vec<&str> = new_content.lines().collect();
    let lcs = compute_lcs(&old_lines, &new_lines);

    let mut hunks = Vec::new();
    let mut old_idx = 0;
    let mut new_idx = 0;
    let mut lcs_idx = 0;

    while old_idx < old_lines.len() || new_idx < new_lines.len() {
        if lcs_idx < lcs.len() && old_idx == lcs[lcs_idx].0 && new_idx == lcs[lcs_idx].1 {
            old_idx += 1;
            new_idx += 1;
            lcs_idx += 1;
        } else {
            let next_old = if lcs_idx < lcs.len() {
                lcs[lcs_idx].0
            } else {
                old_lines.len()
            };
            let next_new = if lcs_idx < lcs.len() {
                lcs[lcs_idx].1
            } else {
                new_lines.len()
            };
            hunks.push(Hunk {
                old_start: old_idx,
                old_end: next_old,
                new_start: new_idx,
                new_end: next_new,
            });
            old_idx = next_old;
            new_idx = next_new;
        }
    }

    hunks
}

/// Rebuild file content applying only the hunks marked approved.
///
/// `approved[i]` corresponds to `compute_hunks(old_content, new_content)[i]`.
/// A rejected hunk keeps its original (old) lines; an approved hunk takes the
/// new lines. Any index missing from `approved` is treated as **rejected**, so
/// a truncated decision list can never apply a change the user did not accept.
///
/// Approving every hunk reproduces `new_content` byte-for-byte; rejecting every
/// hunk reproduces `old_content` byte-for-byte.
pub fn merge_hunks(old_content: &str, new_content: &str, approved: &[bool]) -> String {
    let hunks = compute_hunks(old_content, new_content);
    if hunks.is_empty() {
        return new_content.to_string();
    }

    // Exact round-trips for the all-or-nothing cases, which also preserves
    // trailing-newline and line-ending details the line split would drop.
    if approved.len() == hunks.len() {
        if approved.iter().all(|approved| *approved) {
            return new_content.to_string();
        }
        if approved.iter().all(|approved| !*approved) {
            return old_content.to_string();
        }
    }

    let old_lines: Vec<&str> = old_content.lines().collect();
    let new_lines: Vec<&str> = new_content.lines().collect();
    let mut merged: Vec<&str> = Vec::new();
    let mut old_cursor = 0;

    for (index, hunk) in hunks.iter().enumerate() {
        // Unchanged context between the previous hunk and this one.
        merged.extend_from_slice(&old_lines[old_cursor..hunk.old_start]);
        if approved.get(index).copied().unwrap_or(false) {
            merged.extend_from_slice(&new_lines[hunk.new_start..hunk.new_end]);
        } else {
            merged.extend_from_slice(&old_lines[hunk.old_start..hunk.old_end]);
        }
        old_cursor = hunk.old_end;
    }
    merged.extend_from_slice(&old_lines[old_cursor..]);

    let mut result = merged.join("\n");
    if new_content.ends_with('\n') && !result.is_empty() {
        result.push('\n');
    }
    result
}

/// Render a single hunk for per-hunk review, with a `[i/n]` progress header.
pub fn render_hunk_lines(
    diff: &DiffData,
    hunk: &Hunk,
    index: usize,
    total: usize,
    max_width: u16,
) -> Vec<Line<'static>> {
    let width = max_width as usize;
    let mut lines = Vec::new();

    lines.push(Line::from(vec![Span::styled(
        format!(
            " Hunk {}/{total} of {} (-{} +{})",
            index + 1,
            diff.path,
            hunk.removed_count(),
            hunk.added_count()
        ),
        Style::default()
            .fg(theme::border_focused_color())
            .add_modifier(Modifier::BOLD),
    )]));

    let old_lines: Vec<&str> = diff.old_content.lines().collect();
    let new_lines: Vec<&str> = diff.new_content.lines().collect();

    for line in &old_lines[hunk.old_start..hunk.old_end] {
        let display = truncate_line(line, width.saturating_sub(4));
        lines.push(Line::from(vec![Span::styled(
            format!("  - {display}"),
            Style::default().fg(theme::diff_remove_color()),
        )]));
    }
    for line in &new_lines[hunk.new_start..hunk.new_end] {
        let display = truncate_line(line, width.saturating_sub(4));
        lines.push(Line::from(vec![Span::styled(
            format!("  + {display}"),
            Style::default().fg(theme::diff_add_color()),
        )]));
    }

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

/// Render a unified diff as styled terminal lines.
pub fn render_diff_lines(diff: &DiffData, max_width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let width = max_width as usize;

    if width >= 160 && !diff.is_new_file {
        return render_side_by_side_diff_lines(diff, width);
    }

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

/// Render a side-by-side diff for wide terminal layouts.
fn render_side_by_side_diff_lines(diff: &DiffData, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let header_style = Style::default()
        .fg(theme::border_focused_color())
        .add_modifier(Modifier::BOLD);
    let old_width = width.saturating_sub(9) / 2;
    let new_width = width.saturating_sub(9).saturating_sub(old_width);

    let old_header = truncate_line(&format!("--- {}", diff.path), old_width);
    let new_header = truncate_line(&format!("+++ {}", diff.path), new_width);
    lines.push(side_by_side_line(
        DiffCell {
            prefix: "  ",
            text: &old_header,
            style: header_style,
        },
        DiffCell {
            prefix: "  ",
            text: &new_header,
            style: header_style,
        },
        old_width,
        new_width,
    ));

    let old_lines: Vec<&str> = diff.old_content.lines().collect();
    let new_lines: Vec<&str> = diff.new_content.lines().collect();
    let lcs = compute_lcs(&old_lines, &new_lines);
    let mut old_idx = 0;
    let mut new_idx = 0;
    let mut lcs_idx = 0;

    while old_idx < old_lines.len() || new_idx < new_lines.len() {
        if lcs_idx < lcs.len() && old_idx == lcs[lcs_idx].0 && new_idx == lcs[lcs_idx].1 {
            lines.push(side_by_side_line(
                DiffCell {
                    prefix: "  ",
                    text: old_lines[old_idx],
                    style: Style::default().add_modifier(Modifier::DIM),
                },
                DiffCell {
                    prefix: "  ",
                    text: new_lines[new_idx],
                    style: Style::default().add_modifier(Modifier::DIM),
                },
                old_width,
                new_width,
            ));
            old_idx += 1;
            new_idx += 1;
            lcs_idx += 1;
        } else {
            let next_old = if lcs_idx < lcs.len() {
                lcs[lcs_idx].0
            } else {
                old_lines.len()
            };
            let next_new = if lcs_idx < lcs.len() {
                lcs[lcs_idx].1
            } else {
                new_lines.len()
            };

            while old_idx < next_old || new_idx < next_new {
                let old = (old_idx < next_old).then_some(old_lines[old_idx]);
                let new = (new_idx < next_new).then_some(new_lines[new_idx]);
                lines.push(side_by_side_line(
                    DiffCell {
                        prefix: old.map_or("  ", |_| "- "),
                        text: old.unwrap_or(""),
                        style: Style::default().fg(theme::diff_remove_color()),
                    },
                    DiffCell {
                        prefix: new.map_or("  ", |_| "+ "),
                        text: new.unwrap_or(""),
                        style: Style::default().fg(theme::diff_add_color()),
                    },
                    old_width,
                    new_width,
                ));
                if old.is_some() {
                    old_idx += 1;
                }
                if new.is_some() {
                    new_idx += 1;
                }
            }
        }
    }

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

fn side_by_side_line(
    old: DiffCell<'_>,
    new: DiffCell<'_>,
    old_width: usize,
    new_width: usize,
) -> Line<'static> {
    let old_display = truncate_line(old.text, old_width);
    let new_display = truncate_line(new.text, new_width);
    let old_cell = format!("{}{old_display:<old_width$}", old.prefix);
    let new_cell = format!("{}{new_display}", new.prefix);
    Line::from(vec![
        Span::styled(old_cell, old.style),
        Span::styled(" | ", Style::default().add_modifier(Modifier::DIM)),
        Span::styled(new_cell, new.style),
    ])
}

#[derive(Clone, Copy)]
struct DiffCell<'a> {
    prefix: &'a str,
    text: &'a str,
    style: Style,
}

/// Truncate a line to max characters, respecting UTF-8 char boundaries.
fn truncate_line(line: &str, max: usize) -> String {
    truncate_with_ellipsis(line, max)
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
    fn diff_data_new_sets_every_field() {
        let diff = DiffData::new("/tmp/test.rs", true, "", "hello\n");
        assert_eq!(diff.path, "/tmp/test.rs");
        assert!(diff.is_new_file);
        assert_eq!(diff.old_content, "");
        assert_eq!(diff.new_content, "hello\n");
    }

    #[test]
    fn hunk_new_sets_ranges() {
        let hunk = Hunk::new(1, 3, 1, 5);
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_end, 3);
        assert_eq!(hunk.new_start, 1);
        assert_eq!(hunk.new_end, 5);
        assert_eq!(hunk.removed_count(), 2);
        assert_eq!(hunk.added_count(), 4);
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
    fn render_wide_modification_uses_side_by_side_layout() {
        let diff = DiffData {
            path: "/tmp/test.rs".to_string(),
            is_new_file: false,
            old_content: "line1\nold\nline3".to_string(),
            new_content: "line1\nnew\nline3".to_string(),
        };
        let lines = render_diff_lines(&diff, 160);
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered.iter().all(|line| line.contains(" | ")),
            "wide diffs should render every row in two columns: {rendered:?}"
        );
        assert!(
            rendered
                .iter()
                .any(|line| line.contains("- old") && line.contains("+ new")),
            "changed lines should be paired in the same side-by-side row: {rendered:?}"
        );
    }

    #[test]
    fn render_wide_new_file_keeps_unified_additions() {
        let diff = DiffData {
            path: "/tmp/test.rs".to_string(),
            is_new_file: true,
            old_content: String::new(),
            new_content: "line1\nline2".to_string(),
        };
        let lines = render_diff_lines(&diff, 160);
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered.iter().all(|line| !line.contains(" | ")),
            "new files should stay in the compact unified layout: {rendered:?}"
        );
        assert!(rendered.iter().any(|line| line.contains("+ line1")));
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
    fn compute_hunks_identical_content_has_no_hunks() {
        assert!(compute_hunks("a\nb\nc", "a\nb\nc").is_empty());
    }

    #[test]
    fn compute_hunks_groups_separate_changes() {
        // Two changed regions separated by an unchanged line.
        let hunks = compute_hunks("a\nold1\nb\nold2\nc", "a\nnew1\nb\nnew2\nc");
        assert_eq!(hunks.len(), 2, "expected two hunks, got {hunks:?}");
        assert_eq!(hunks[0].removed_count(), 1);
        assert_eq!(hunks[0].added_count(), 1);
        assert_eq!(hunks[1].removed_count(), 1);
        assert_eq!(hunks[1].added_count(), 1);
    }

    #[test]
    fn compute_hunks_contiguous_change_is_one_hunk() {
        let hunks = compute_hunks("a\nold1\nold2\nb", "a\nnew1\nnew2\nb");
        assert_eq!(hunks.len(), 1, "expected one hunk, got {hunks:?}");
        assert_eq!(hunks[0].removed_count(), 2);
        assert_eq!(hunks[0].added_count(), 2);
    }

    #[test]
    fn compute_hunks_handles_pure_insertion_and_deletion() {
        let inserted = compute_hunks("a\nc", "a\nb\nc");
        assert_eq!(inserted.len(), 1);
        assert_eq!(inserted[0].removed_count(), 0);
        assert_eq!(inserted[0].added_count(), 1);

        let deleted = compute_hunks("a\nb\nc", "a\nc");
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].removed_count(), 1);
        assert_eq!(deleted[0].added_count(), 0);
    }

    #[test]
    fn merge_hunks_approving_all_reproduces_new_content() {
        let old = "a\nold1\nb\nold2\nc\n";
        let new = "a\nnew1\nb\nnew2\nc\n";
        assert_eq!(merge_hunks(old, new, &[true, true]), new);
    }

    #[test]
    fn merge_hunks_rejecting_all_reproduces_old_content() {
        let old = "a\nold1\nb\nold2\nc\n";
        let new = "a\nnew1\nb\nnew2\nc\n";
        assert_eq!(merge_hunks(old, new, &[false, false]), old);
    }

    #[test]
    fn merge_hunks_applies_only_approved_hunks() {
        let old = "a\nold1\nb\nold2\nc\n";
        let new = "a\nnew1\nb\nnew2\nc\n";
        // Approve the first hunk, reject the second.
        assert_eq!(
            merge_hunks(old, new, &[true, false]),
            "a\nnew1\nb\nold2\nc\n"
        );
        // And the mirror image.
        assert_eq!(
            merge_hunks(old, new, &[false, true]),
            "a\nold1\nb\nnew2\nc\n"
        );
    }

    #[test]
    fn merge_hunks_applies_approved_insertion_and_keeps_rejected_deletion() {
        let old = "keep\ndelete_me\ntail\n";
        let new = "keep\ninserted\ntail\n";
        let hunks = compute_hunks(old, new);
        assert_eq!(hunks.len(), 1);
        assert_eq!(merge_hunks(old, new, &[false]), old);
        assert_eq!(merge_hunks(old, new, &[true]), new);
    }

    #[test]
    fn merge_hunks_missing_decisions_default_to_rejected() {
        // A truncated decision list must never apply an unreviewed hunk.
        let old = "a\nold1\nb\nold2\nc\n";
        let new = "a\nnew1\nb\nnew2\nc\n";
        assert_eq!(merge_hunks(old, new, &[true]), "a\nnew1\nb\nold2\nc\n");
        assert_eq!(merge_hunks(old, new, &[]), old);
    }

    #[test]
    fn merge_hunks_preserves_absent_trailing_newline() {
        let old = "a\nold1\nb\nold2";
        let new = "a\nnew1\nb\nnew2";
        assert_eq!(merge_hunks(old, new, &[true, false]), "a\nnew1\nb\nold2");
    }

    #[test]
    fn merge_hunks_identical_content_returns_new_content() {
        assert_eq!(merge_hunks("a\nb\n", "a\nb\n", &[]), "a\nb\n");
    }

    #[test]
    fn render_hunk_lines_shows_removals_additions_and_progress() {
        let diff = DiffData {
            path: "/tmp/test.rs".to_string(),
            is_new_file: false,
            old_content: "a\nold\nb".to_string(),
            new_content: "a\nnew\nb".to_string(),
        };
        let hunks = compute_hunks(&diff.old_content, &diff.new_content);
        let lines = render_hunk_lines(&diff, &hunks[0], 0, hunks.len(), 80);
        let rendered = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        assert!(
            rendered[0].contains("Hunk 1/1"),
            "header should show progress: {rendered:?}"
        );
        assert!(rendered.iter().any(|line| line.contains("- old")));
        assert!(rendered.iter().any(|line| line.contains("+ new")));
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
    fn truncate_line_multibyte_does_not_panic() {
        // Each CJK character is 3 bytes in UTF-8; a byte-index slice at an
        // odd offset would land mid-character and panic.
        let line = "文".repeat(30);
        let result = truncate_line(&line, 20);
        assert!(result.ends_with("..."));
        assert!(result.chars().count() <= 20);
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
