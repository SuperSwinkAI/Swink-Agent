//! Tests for `help_panel`.
#![cfg(test)]

use super::*;

#[test]
fn new_panel_is_hidden() {
    let panel = HelpPanel::new();
    assert!(!panel.visible);
    assert_eq!(panel.width(), 0);
}

#[test]
fn toggle_makes_visible() {
    let mut panel = HelpPanel::new();
    panel.toggle();
    assert!(panel.visible);
    assert_eq!(panel.width(), HELP_PANEL_WIDTH);
}

#[test]
fn toggle_twice_hides() {
    let mut panel = HelpPanel::new();
    panel.toggle();
    panel.toggle();
    assert!(!panel.visible);
    assert_eq!(panel.width(), 0);
}

#[test]
fn help_lines_not_empty() {
    let lines = help_lines();
    assert!(!lines.is_empty());
}
