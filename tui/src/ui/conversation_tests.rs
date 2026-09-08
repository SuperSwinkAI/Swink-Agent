//! Tests for `conversation`.
#![cfg(test)]

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::text::Line;

use super::{CachedMessage, ConversationView, RenderCache, message_fingerprint};
use crate::app::{DisplayMessage, MessageRole, Selection};
use crate::theme::ColorMode;

fn msg(role: MessageRole, content: &str) -> DisplayMessage {
    DisplayMessage::new(role, content.to_string())
}

/// Render `messages` into a test terminal at the given width.
fn draw(
    view: &mut ConversationView,
    messages: &[DisplayMessage],
    selection: Option<&Selection>,
    width: u16,
) {
    let mut terminal = Terminal::new(TestBackend::new(width, 12)).expect("test backend");
    terminal
        .draw(|frame| {
            view.render(
                frame,
                frame.area(),
                messages,
                false,
                false,
                true,
                None,
                selection,
            );
        })
        .expect("draw");
}

/// Render `messages` with thinking visible, toggling hidden-channels
/// inline rendering on or off via the view's own field.
fn draw_with_thinking(
    view: &mut ConversationView,
    messages: &[DisplayMessage],
    show_hidden_channels: bool,
    width: u16,
) {
    view.show_hidden_channels = show_hidden_channels;
    let mut terminal = Terminal::new(TestBackend::new(width, 12)).expect("test backend");
    terminal
        .draw(|frame| {
            view.render(frame, frame.area(), messages, true, false, true, None, None);
        })
        .expect("draw");
}

fn cached_lines_ptr(view: &ConversationView, idx: usize) -> *const Line<'static> {
    view.cache.entries[idx]
        .as_ref()
        .expect("cache entry should be populated")
        .lines
        .as_ptr()
}

fn cached_text(view: &ConversationView, idx: usize) -> String {
    view.cache.entries[idx]
        .as_ref()
        .expect("cache entry should be populated")
        .lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect()
}

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

// --- message fingerprint (cache keying) ---

#[test]
fn fingerprint_stable_for_identical_input() {
    let a = msg(MessageRole::Assistant, "hello **world**");
    assert_eq!(
        message_fingerprint(&a, false),
        message_fingerprint(&a, false)
    );
}

#[test]
fn fingerprint_changes_with_content() {
    let a = msg(MessageRole::Assistant, "hello");
    let b = msg(MessageRole::Assistant, "hello!");
    assert_ne!(
        message_fingerprint(&a, false),
        message_fingerprint(&b, false)
    );
}

#[test]
fn fingerprint_changes_with_role() {
    let a = msg(MessageRole::User, "same text");
    let b = msg(MessageRole::Error, "same text");
    assert_ne!(
        message_fingerprint(&a, false),
        message_fingerprint(&b, false)
    );
}

#[test]
fn fingerprint_changes_with_tool_block_selection() {
    let a = msg(MessageRole::ToolResult, "output");
    assert_ne!(
        message_fingerprint(&a, false),
        message_fingerprint(&a, true)
    );
}

#[test]
fn fingerprint_changes_when_collapsed() {
    let expanded = msg(MessageRole::ToolResult, "output");
    let collapsed = msg(MessageRole::ToolResult, "output").with_collapsed("summary");
    assert_ne!(
        message_fingerprint(&expanded, false),
        message_fingerprint(&collapsed, false)
    );
}

#[test]
fn fingerprint_changes_with_diff_data() {
    let plain = msg(MessageRole::ToolResult, "wrote file");
    let with_diff =
        msg(MessageRole::ToolResult, "wrote file").with_diff_data(crate::ui::diff::DiffData {
            path: "src/main.rs".to_string(),
            is_new_file: false,
            old_content: "a".to_string(),
            new_content: "b".to_string(),
        });
    assert_ne!(
        message_fingerprint(&plain, false),
        message_fingerprint(&with_diff, false)
    );
}

// --- render cache invalidation ---

#[test]
fn cache_sync_clears_on_global_change_and_resizes() {
    let entry = || {
        Some(CachedMessage {
            fingerprint: 1,
            lines: Vec::new(),
        })
    };
    let mut cache = RenderCache::new();
    cache.sync(40, ColorMode::Custom, false, false, 2);
    assert_eq!(cache.entries.len(), 2);

    cache.entries[0] = entry();
    cache.sync(40, ColorMode::Custom, false, false, 2);
    assert!(cache.entries[0].is_some(), "same globals keep entries");

    cache.sync(39, ColorMode::Custom, false, false, 2);
    assert!(cache.entries[0].is_none(), "width change clears entries");

    cache.entries[0] = entry();
    cache.sync(39, ColorMode::Custom, true, false, 2);
    assert!(
        cache.entries[0].is_none(),
        "show_thinking change clears entries"
    );

    cache.entries[0] = entry();
    cache.sync(39, ColorMode::Custom, true, true, 2);
    assert!(
        cache.entries[0].is_none(),
        "show_hidden_channels change clears entries"
    );

    cache.entries[0] = entry();
    cache.sync(39, ColorMode::MonoWhite, true, true, 2);
    assert!(
        cache.entries[0].is_none(),
        "color mode change clears entries"
    );

    cache.sync(39, ColorMode::MonoWhite, true, true, 1);
    assert_eq!(cache.entries.len(), 1, "shrinking message list truncates");
}

#[test]
fn hidden_channels_off_shows_collapsed_placeholder() {
    let mut view = ConversationView::new();
    let messages = vec![msg(MessageRole::Assistant, "reply").with_thinking("secret reasoning")];

    draw_with_thinking(&mut view, &messages, false, 40);

    let text = cached_text(&view, 0);
    assert!(text.contains("[thinking...]"), "{text}");
    assert!(!text.contains("secret reasoning"), "{text}");
}

#[test]
fn hidden_channels_on_renders_reasoning_inline() {
    let mut view = ConversationView::new();
    let messages = vec![msg(MessageRole::Assistant, "reply").with_thinking("secret reasoning")];

    draw_with_thinking(&mut view, &messages, true, 40);

    let text = cached_text(&view, 0);
    assert!(text.contains("secret reasoning"), "{text}");
    assert!(!text.contains("[thinking...]"), "{text}");
}

#[test]
fn render_caches_messages_and_reuses_entries() {
    let mut view = ConversationView::new();
    let messages = vec![
        msg(MessageRole::User, "first"),
        msg(MessageRole::Assistant, "second"),
    ];
    draw(&mut view, &messages, None, 40);
    assert_eq!(view.cache.entries.len(), 2);
    let ptr0 = cached_lines_ptr(&view, 0);
    let ptr1 = cached_lines_ptr(&view, 1);

    draw(&mut view, &messages, None, 40);
    assert_eq!(
        cached_lines_ptr(&view, 0),
        ptr0,
        "unchanged message should reuse its cached lines"
    );
    assert_eq!(cached_lines_ptr(&view, 1), ptr1);
}

#[test]
fn render_rebuilds_only_the_changed_message() {
    let mut view = ConversationView::new();
    let mut messages = vec![
        msg(MessageRole::User, "first"),
        msg(MessageRole::Assistant, "second"),
    ];
    draw(&mut view, &messages, None, 40);
    let ptr0 = cached_lines_ptr(&view, 0);

    messages[1].content.push_str(" more");
    draw(&mut view, &messages, None, 40);
    assert_eq!(
        cached_lines_ptr(&view, 0),
        ptr0,
        "untouched message stays cached"
    );
    assert!(
        cached_text(&view, 1).contains("second more"),
        "changed message re-renders with new content"
    );
}

#[test]
fn render_width_change_rewraps_cached_lines() {
    let mut view = ConversationView::new();
    let long = "word ".repeat(30);
    let messages = vec![msg(MessageRole::User, &long)];

    draw(&mut view, &messages, None, 80);
    let lines_wide = view.cache.entries[0].as_ref().unwrap().lines.len();

    draw(&mut view, &messages, None, 30);
    let lines_narrow = view.cache.entries[0].as_ref().unwrap().lines.len();
    assert!(
        lines_narrow > lines_wide,
        "narrower width should wrap into more lines ({lines_narrow} vs {lines_wide})"
    );
}

#[test]
fn render_does_not_cache_streaming_message() {
    let mut view = ConversationView::new();
    let messages = vec![
        msg(MessageRole::User, "prompt"),
        msg(MessageRole::Assistant, "partial answer").with_is_streaming(true),
    ];
    draw(&mut view, &messages, None, 40);

    assert!(view.cache.entries[0].is_some(), "settled message is cached");
    assert!(
        view.cache.entries[1].is_none(),
        "streaming message must not be cached"
    );
}

// --- selection cell capture ---

#[test]
fn render_without_selection_skips_cell_capture() {
    let mut view = ConversationView::new();
    let messages = vec![msg(MessageRole::User, "hello")];
    draw(&mut view, &messages, None, 40);

    assert!(
        view.visible_cells.is_empty(),
        "no selection means no per-cell capture"
    );
}

#[test]
fn render_with_selection_captures_cells_then_clears() {
    let mut view = ConversationView::new();
    let messages = vec![msg(MessageRole::User, "hello")];
    let selection = Selection {
        anchor: (0, 0),
        cursor: (0, 5),
        dragging: true,
    };

    draw(&mut view, &messages, Some(&selection), 40);
    assert!(!view.visible_cells.is_empty(), "selection captures cells");
    assert_eq!(
        view.selection_text(&selection).as_deref(),
        Some("You"),
        "captured cells back the copied text"
    );

    draw(&mut view, &messages, None, 40);
    assert!(
        view.visible_cells.is_empty(),
        "dropping the selection clears the stale capture"
    );
}
