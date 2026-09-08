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
#[path = "render_helpers_tests.rs"]
mod tests;
