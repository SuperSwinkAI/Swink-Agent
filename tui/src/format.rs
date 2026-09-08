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

/// Format a context window gauge as a 10-character bar with percentage.
///
/// Returns a tuple of (bar string, fill percentage) where the bar looks like
/// `[████████░░]` and the percentage is 0.0 to 100.0+.
#[must_use]
pub fn format_context_gauge(tokens_used: u64, budget: u64) -> (String, f32) {
    if budget == 0 {
        return ("[ no limit ]".to_string(), 0.0);
    }
    #[allow(clippy::cast_precision_loss)]
    let pct = (tokens_used as f32 / budget as f32) * 100.0;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let filled = ((pct / 100.0) * 10.0).round().min(10.0) as usize;
    let empty = 10 - filled;
    let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(empty));
    (bar, pct)
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
