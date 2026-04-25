//! Rendering and formatting helpers local to the app module.

/// Extract all fenced code blocks from markdown text and concatenate them.
pub(super) fn extract_code_blocks(text: &str) -> Option<String> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut current = Vec::new();

    for line in text.lines() {
        if line.starts_with("```") {
            if in_block {
                blocks.push(current.join("\n"));
                current.clear();
                in_block = false;
            } else {
                in_block = true;
            }
        } else if in_block {
            current.push(line);
        }
    }

    if blocks.is_empty() {
        None
    } else {
        Some(blocks.join("\n\n"))
    }
}

/// Get current Unix timestamp.
pub(super) fn timestamp_now() -> u64 {
    swink_agent::now_timestamp()
}

#[cfg(test)]
mod tests {
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
}
