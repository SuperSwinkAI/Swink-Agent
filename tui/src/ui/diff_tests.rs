//! Tests for `diff`.
#![cfg(test)]

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
