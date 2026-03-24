use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::TuiConfig;

use super::super::*;
use super::helpers::*;

#[tokio::test]
async fn tool_result_has_collapsed_fields() {
    let msg = make_tool_result_message("file contents here\nsecond line");
    assert!(!msg.collapsed, "tool result starts expanded");
    assert_eq!(msg.summary, "file contents here");
    assert!(!msg.user_expanded);
    assert!(msg.expanded_at.is_some());
}

#[tokio::test]
async fn toggle_collapse_toggles_state() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_tool_result_message("tool output"));

    assert!(!app.messages[0].collapsed);

    app.toggle_collapse(0);
    assert!(app.messages[0].collapsed);
    assert!(!app.messages[0].user_expanded);

    app.toggle_collapse(0);
    assert!(!app.messages[0].collapsed);
    assert!(app.messages[0].user_expanded);
}

#[tokio::test]
async fn toggle_collapse_non_tool_is_noop() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_user_message("hello"));

    app.toggle_collapse(0);
    assert!(
        !app.messages[0].collapsed,
        "user message should not collapse"
    );
}

#[tokio::test]
async fn auto_collapse_after_timeout() {
    let mut app = App::new(TuiConfig::default());
    let mut msg = make_tool_result_message("tool output");
    msg.expanded_at = Some(instant_secs_ago(11));
    app.messages.push(msg);

    assert!(!app.messages[0].collapsed);

    app.tick();

    assert!(
        app.messages[0].collapsed,
        "tool result should auto-collapse after 10 seconds"
    );
}

#[tokio::test]
async fn user_expanded_prevents_auto_collapse() {
    let mut app = App::new(TuiConfig::default());
    let mut msg = make_tool_result_message("tool output");
    msg.expanded_at = Some(instant_secs_ago(11));
    msg.user_expanded = true;
    app.messages.push(msg);

    app.tick();

    assert!(
        !app.messages[0].collapsed,
        "user-expanded tool result should not auto-collapse"
    );
}

#[tokio::test]
async fn select_next_tool_block_navigates() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_user_message("hello"));
    app.messages.push(make_tool_result_message("tool 1"));
    app.messages.push(make_user_message("world"));
    app.messages.push(make_tool_result_message("tool 2"));

    assert_eq!(app.selected_tool_block, None);

    assert!(app.select_next_tool_block());
    assert_eq!(app.selected_tool_block, Some(1));

    assert!(app.select_next_tool_block());
    assert_eq!(app.selected_tool_block, Some(3));

    assert!(app.select_next_tool_block());
    assert_eq!(app.selected_tool_block, Some(3));
}

#[tokio::test]
async fn select_prev_tool_block_navigates() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_user_message("hello"));
    app.messages.push(make_tool_result_message("tool 1"));
    app.messages.push(make_user_message("world"));
    app.messages.push(make_tool_result_message("tool 2"));

    assert!(app.select_prev_tool_block());
    assert_eq!(app.selected_tool_block, Some(3));

    assert!(app.select_prev_tool_block());
    assert_eq!(app.selected_tool_block, Some(1));

    assert!(app.select_prev_tool_block());
    assert_eq!(app.selected_tool_block, Some(1));
}

#[tokio::test]
async fn f2_toggles_most_recent_tool_block() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_user_message("hello"));
    app.messages.push(make_tool_result_message("tool 1"));
    app.messages.push(make_user_message("world"));
    app.messages.push(make_tool_result_message("tool 2"));

    assert_eq!(app.selected_tool_block, None);
    assert!(!app.messages[3].collapsed);

    let key = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert_eq!(app.selected_tool_block, Some(3));
    assert!(
        app.messages[3].collapsed,
        "most recent tool block should collapse"
    );
}

#[tokio::test]
async fn f2_toggles_selected_tool_block() {
    let mut app = App::new(TuiConfig::default());
    app.messages.push(make_tool_result_message("tool 1"));
    app.messages.push(make_user_message("hello"));
    app.messages.push(make_tool_result_message("tool 2"));

    app.selected_tool_block = Some(0);
    assert!(!app.messages[0].collapsed);

    let key = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
    app.handle_key_event(key);

    assert!(
        app.messages[0].collapsed,
        "selected tool block should collapse"
    );
    assert!(
        !app.messages[2].collapsed,
        "other tool block should stay expanded"
    );
}
