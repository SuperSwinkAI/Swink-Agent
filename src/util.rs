//! Internal utility functions shared across the crate.

/// Return the longest prefix containing at most `max_chars` Unicode scalar values.
#[must_use]
pub fn prefix_chars(s: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }

    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Return the longest suffix containing at most `max_chars` Unicode scalar values.
#[must_use]
pub fn suffix_chars(s: &str, max_chars: usize) -> &str {
    if max_chars == 0 {
        return "";
    }

    let total_chars = s.chars().count();
    if total_chars <= max_chars {
        return s;
    }

    let start_idx = s
        .char_indices()
        .nth(total_chars - max_chars)
        .map_or(0, |(idx, _)| idx);

    &s[start_idx..]
}

/// Get the current Unix timestamp in seconds.
pub fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
#[path = "util_tests.rs"]
mod tests;
