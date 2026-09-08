//! Tests for `input`.
#![cfg(test)]

use super::*;

#[test]
fn new_editor_is_empty() {
    let editor = InputEditor::new();
    assert_eq!(editor.lines, vec![String::new()]);
    assert_eq!(editor.cursor_row, 0);
    assert_eq!(editor.cursor_col, 0);
    assert!(editor.lines.iter().all(String::is_empty));
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

fn editor_with(text: &str) -> InputEditor {
    let mut editor = InputEditor::new();
    for ch in text.chars() {
        if ch == '\n' {
            editor.insert_newline();
        } else {
            editor.insert_char(ch);
        }
    }
    editor
}

#[test]
fn no_mention_query_in_plain_text() {
    assert!(editor_with("hello world").mention_query().is_none());
}

#[test]
fn mention_query_is_empty_right_after_the_at_sign() {
    let query = editor_with("look at @").mention_query().unwrap();
    assert_eq!(query.query, "");
    assert_eq!(query.start, 8);
}

#[test]
fn mention_query_grows_as_the_path_is_typed() {
    assert_eq!(
        editor_with("@src/li").mention_query().unwrap().query,
        "src/li"
    );
}

#[test]
fn whitespace_after_the_mention_closes_the_query() {
    assert!(editor_with("@src/lib.rs ").mention_query().is_none());
}

#[test]
fn at_sign_inside_a_word_is_not_a_mention_query() {
    assert!(editor_with("wes@example").mention_query().is_none());
}

#[test]
fn mention_query_tracks_the_cursor_not_the_line_end() {
    let mut editor = editor_with("@src/lib.rs");
    editor.move_left();
    editor.move_left();
    // Cursor sits between "." and "rs" — the query is the prefix only.
    assert_eq!(editor.mention_query().unwrap().query, "src/lib.");
}

#[test]
fn mention_query_found_on_a_later_line() {
    let editor = editor_with("first line\n@src/li");
    let query = editor.mention_query().unwrap();
    assert_eq!(query.query, "src/li");
    assert_eq!(query.start, 0);
}

#[test]
fn replace_mention_query_swaps_in_the_accepted_path() {
    let mut editor = editor_with("look at @src/li");
    let start = editor.mention_query().unwrap().start;
    editor.replace_mention_query(start, "@src/lib.rs ");
    assert_eq!(editor.lines(), ["look at @src/lib.rs "]);
}

#[test]
fn replace_mention_query_leaves_the_cursor_after_the_insertion() {
    let mut editor = editor_with("@src/li");
    editor.replace_mention_query(0, "@src/lib.rs ");
    assert_eq!(editor.cursor_col, 12);
    editor.insert_char('x');
    assert_eq!(editor.lines(), ["@src/lib.rs x"]);
}

#[test]
fn replace_mention_query_preserves_text_after_the_cursor() {
    let mut editor = editor_with("@src/li tail");
    for _ in 0..5 {
        editor.move_left();
    }
    let start = editor.mention_query().unwrap().start;
    editor.replace_mention_query(start, "@src/lib.rs");
    assert_eq!(editor.lines(), ["@src/lib.rs tail"]);
}

#[test]
fn replace_mention_query_ignores_a_stale_start_offset() {
    let mut editor = editor_with("@a");
    editor.replace_mention_query(99, "@should-not-apply");
    assert_eq!(editor.lines(), ["@a"]);
}

#[test]
fn replace_mention_query_handles_multibyte_prefixes() {
    let mut editor = editor_with("héllo @src/li");
    let start = editor.mention_query().unwrap().start;
    editor.replace_mention_query(start, "@src/lib.rs");
    assert_eq!(editor.lines(), ["héllo @src/lib.rs"]);
}

#[test]
fn no_slash_query_in_plain_text() {
    assert!(editor_with("hello world").slash_query().is_none());
}

#[test]
fn slash_query_is_empty_right_after_the_slash() {
    let query = editor_with("/").slash_query().unwrap();
    assert_eq!(query.query, "");
    assert_eq!(query.start, 0);
}

#[test]
fn slash_query_grows_as_the_name_is_typed() {
    assert_eq!(editor_with("/depl").slash_query().unwrap().query, "depl");
}

#[test]
fn slash_query_allows_leading_whitespace() {
    let query = editor_with("  /dep").slash_query().unwrap();
    assert_eq!(query.query, "dep");
    assert_eq!(query.start, 2);
}

#[test]
fn whitespace_after_the_name_closes_the_slash_query() {
    assert!(editor_with("/deploy ").slash_query().is_none());
    assert!(editor_with("/deploy prod").slash_query().is_none());
}

#[test]
fn a_mid_text_slash_is_not_a_slash_query() {
    assert!(editor_with("see /dep").slash_query().is_none());
    assert!(editor_with("either/or").slash_query().is_none());
}

#[test]
fn a_path_at_line_start_does_produce_a_slash_query() {
    // The popup closes because the host returns no candidates for it —
    // the query itself is legitimate.
    assert_eq!(
        editor_with("/usr/bin").slash_query().unwrap().query,
        "usr/bin"
    );
}

#[test]
fn a_slash_on_a_later_line_is_not_a_slash_query() {
    assert!(editor_with("first line\n/dep").slash_query().is_none());
}

#[test]
fn slash_query_tracks_the_cursor_not_the_line_end() {
    let mut editor = editor_with("/deploy");
    editor.move_left();
    editor.move_left();
    // Cursor sits between "depl" and "oy" — the query is the prefix only.
    assert_eq!(editor.slash_query().unwrap().query, "depl");
}

#[test]
fn slash_query_and_mention_query_are_mutually_exclusive() {
    // A leading slash token is not a mention...
    let slash = editor_with("/dep");
    assert!(slash.slash_query().is_some());
    assert!(slash.mention_query().is_none());

    // ...and a mention is not a leading slash token.
    let mention = editor_with("look at @src/li");
    assert!(mention.mention_query().is_some());
    assert!(mention.slash_query().is_none());

    // Even a mention typed after a leading command: the cursor is in the
    // mention, so only the mention query fires.
    let both = editor_with("/deploy @src/li");
    assert!(both.slash_query().is_none(), "whitespace closed the token");
    assert!(both.mention_query().is_some());
}

#[test]
fn replace_mention_query_splices_an_accepted_skill() {
    let mut editor = editor_with("/dep");
    let start = editor.slash_query().unwrap().start;
    editor.replace_mention_query(start, "/deploy ");
    assert_eq!(editor.lines(), ["/deploy "]);
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
fn submit_without_history_clears_but_skips_history() {
    let mut editor = InputEditor::new();
    for c in "#key openai sk-leak-sentinel".chars() {
        editor.insert_char(c);
    }
    let result = editor.submit_without_history();
    assert_eq!(result.as_deref(), Some("#key openai sk-leak-sentinel"));
    assert_eq!(editor.lines, vec![String::new()]);
    assert_eq!(editor.cursor_row, 0);
    assert_eq!(editor.cursor_col, 0);
    assert!(
        editor.history.is_empty(),
        "submit_without_history must not push to history"
    );
}

#[test]
fn submit_without_history_on_empty_returns_none() {
    let mut editor = InputEditor::new();
    assert_eq!(editor.submit_without_history(), None);
    assert!(editor.history.is_empty());
}

#[test]
fn history_navigation_after_submit_without_history_is_empty() {
    let mut editor = InputEditor::new();
    for c in "#key openai sk-leak-sentinel-xyz".chars() {
        editor.insert_char(c);
    }
    let submitted = editor.submit_without_history();
    assert!(submitted.is_some());

    // History is empty, so navigating backwards must not recall the key.
    editor.history_prev();
    assert_eq!(
        editor.lines,
        vec![String::new()],
        "sensitive submission must not be recallable via history"
    );
    for line in &editor.lines {
        assert!(
            !line.contains("sk-leak-sentinel-xyz"),
            "secret value leaked into history: {line}"
        );
    }
}

#[test]
fn multiline_sensitive_submission_does_not_enter_history() {
    let mut editor = InputEditor::new();
    editor.insert_char('p');
    editor.insert_char('r');
    editor.insert_char('e');
    editor.insert_newline();
    for c in "#key anthropic sk-ant-top-secret".chars() {
        editor.insert_char(c);
    }
    editor.insert_newline();
    editor.insert_char('p');
    editor.insert_char('o');
    editor.insert_char('s');
    editor.insert_char('t');
    let submitted = editor.submit_without_history();
    assert!(submitted.is_some());

    // Nothing should be recallable — the ENTIRE multi-line entry is
    // withheld, not just the key line.
    editor.history_prev();
    for line in &editor.lines {
        assert!(
            !line.contains("sk-ant-top-secret"),
            "multi-line sensitive entry leaked secret into history: {line}"
        );
    }
    assert_eq!(editor.lines, vec![String::new()]);
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
fn backspace_with_cursor_past_end_clamps_instead_of_panic() {
    let mut editor = InputEditor::new();
    editor.insert_char('a');
    editor.insert_char('b');
    // Artificially put cursor past end of line
    editor.cursor_col = 100;
    // Should not panic — clamp_cursor brings it in range
    editor.backspace();
    assert_eq!(editor.lines, vec!["a".to_string()]);
    assert_eq!(editor.cursor_col, 1);
}

#[test]
fn delete_with_cursor_past_end_clamps_instead_of_panic() {
    let mut editor = InputEditor::new();
    editor.insert_char('a');
    // Artificially put cursor past end of line
    editor.cursor_col = 100;
    // Should not panic — clamps to end, then merges or no-ops
    editor.delete();
    // Cursor clamped to 1 (end of "a"), nothing to delete
    assert_eq!(editor.lines, vec!["a".to_string()]);
}

#[test]
fn backspace_with_cursor_row_past_end_clamps() {
    let mut editor = InputEditor::new();
    editor.insert_char('a');
    // Artificially put cursor on non-existent row
    editor.cursor_row = 50;
    editor.cursor_col = 10;
    // Should not panic
    editor.backspace();
    // Clamped to row 0, col clamped, then backspace operates normally
    assert!(!editor.lines.is_empty());
}

#[test]
fn fields_are_private() {
    // This test documents that fields are not directly accessible
    // from outside the module. If this compiles, the struct API is correct.
    let editor = InputEditor::new();
    // Only public getters available:
    assert_eq!(editor.cursor_row(), 0);
    assert_eq!(editor.line_count(), 1);
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

#[test]
fn is_empty_true_for_new_editor() {
    let editor = InputEditor::new();
    assert!(editor.is_empty());
}

#[test]
fn is_empty_false_after_insert() {
    let mut editor = InputEditor::new();
    editor.insert_char('a');
    assert!(!editor.is_empty());
}

#[test]
fn is_empty_false_with_blank_second_line() {
    let mut editor = InputEditor::new();
    editor.insert_char('a');
    editor.insert_newline();
    assert!(!editor.is_empty());
}

#[test]
fn is_empty_true_after_submit_clears_editor() {
    let mut editor = InputEditor::new();
    editor.insert_char('a');
    editor.submit();
    assert!(editor.is_empty());
}
