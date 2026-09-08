//! Tests for `render_helpers`.
#![cfg(test)]

use super::*;

#[test]
fn no_code_blocks_returns_none() {
    let text = "Just some plain text\nwith multiple lines\nbut no code blocks.";
    assert_eq!(extract_code_blocks(text), None);
}

#[test]
fn single_code_block() {
    let text = "Some intro text\n```\nhello world\n```\nSome outro text";
    assert_eq!(extract_code_blocks(text), Some("hello world".to_string()));
}

#[test]
fn multiple_code_blocks_are_concatenated() {
    let text = "\
```
first block
```
middle text
```
second block
```
more text
```
third block
```";
    assert_eq!(
        extract_code_blocks(text),
        Some("first block\n\nsecond block\n\nthird block".to_string())
    );
}

#[test]
fn unterminated_code_block() {
    let text = "Some text\n```\nthis block is never closed";
    assert_eq!(extract_code_blocks(text), None);
}

#[test]
fn empty_code_block() {
    let text = "```\n```";
    assert_eq!(extract_code_blocks(text), Some(String::new()));
}

#[test]
fn code_block_with_language_tag() {
    let text = "```rust\nfn main() {}\n```";
    assert_eq!(extract_code_blocks(text), Some("fn main() {}".to_string()));
}
