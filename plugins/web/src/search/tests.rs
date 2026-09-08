//! Tests for `mod`.
#![cfg(test)]

use super::*;

#[test]
fn search_result_is_publicly_constructible() {
    let result = SearchResult {
        title: "Rust".to_string(),
        url: "https://www.rust-lang.org".to_string(),
        snippet: "Systems programming language".to_string(),
    };
    assert_eq!(result.title, "Rust");
}
