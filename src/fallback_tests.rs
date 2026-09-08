//! Tests for `fallback`.
#![cfg(test)]

use super::*;

#[test]
fn empty_fallback() {
    let fb = ModelFallback::new(vec![]);
    assert!(fb.is_empty());
    assert_eq!(fb.len(), 0);
    assert!(fb.models().is_empty());
}

#[test]
fn debug_format() {
    let fb = ModelFallback::new(vec![]);
    let dbg = format!("{fb:?}");
    assert!(dbg.contains("ModelFallback"));
}
