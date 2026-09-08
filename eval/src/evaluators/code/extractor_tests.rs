//! Tests for `extractor`.
#![cfg(test)]

use super::*;

#[test]
fn markdown_fence_extracts_first_block() {
    let response = "Here is the code:\n\n```rust\nfn add(a: i32, b: i32) -> i32 { a + b }\n```\n";
    let out = extract_markdown_fence(response, Some("rust"));
    assert_eq!(
        out.as_deref(),
        Some("fn add(a: i32, b: i32) -> i32 { a + b }")
    );
}

#[test]
fn markdown_fence_skips_non_matching_language() {
    let response = "```python\nprint('hi')\n```\n\n```rust\nfn a() {}\n```\n";
    let out = extract_markdown_fence(response, Some("rust"));
    assert_eq!(out.as_deref(), Some("fn a() {}"));
}

#[test]
fn markdown_fence_ignores_language_when_none() {
    let response = "```\nanything\n```";
    let out = extract_markdown_fence(response, None);
    assert_eq!(out.as_deref(), Some("anything"));
}
