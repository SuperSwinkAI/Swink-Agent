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
#[path = "diff_tests.rs"]
mod tests;
