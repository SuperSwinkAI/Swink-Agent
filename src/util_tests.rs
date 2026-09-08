//! Tests for `util`.
#![cfg(test)]

use super::{prefix_chars, suffix_chars};

#[test]
fn prefix_chars_respects_utf8_boundaries() {
    assert_eq!(prefix_chars("abc🙂def", 4), "abc🙂");
}

#[test]
fn suffix_chars_respects_utf8_boundaries() {
    assert_eq!(suffix_chars("abc🙂def", 4), "🙂def");
}
