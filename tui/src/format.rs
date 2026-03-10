//! Token formatting and elapsed time display.

use std::time::Instant;

/// Format a token count for human-readable display.
///
/// - Below 1,000: shown as-is (e.g. "742")
/// - 1,000–999,999: shown as "X.XK" (e.g. "4.6K")
/// - 1,000,000+: shown as "X.XM" (e.g. "1.2M")
pub fn format_tokens(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        let k = n as f64 / 1_000.0;
        if k < 10.0 {
            format!("{k:.1}K")
        } else {
            format!("{k:.0}K")
        }
    } else {
        let m = n as f64 / 1_000_000.0;
        format!("{m:.1}M")
    }
}

/// Format elapsed time from a session start instant.
///
/// - Under 1 hour: `MM:SS`
/// - 1 hour or more: `HH:MM:SS`
pub fn format_elapsed(start: Instant) -> String {
    let secs = start.elapsed().as_secs();
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;
    if hours > 0 {
        format!("{hours:02}:{mins:02}:{secs:02}")
    } else {
        format!("{mins:02}:{secs:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_tokens() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(42), "42");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1.0K");
        assert_eq!(format_tokens(4_600), "4.6K");
        assert_eq!(format_tokens(15_000), "15K");
        assert_eq!(format_tokens(999_999), "1000K");
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(1_200_000), "1.2M");
    }
}
