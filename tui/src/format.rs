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
mod tests {
    use super::*;

    #[test]
    fn format_tokens_below_thousand() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(1), "1");
        assert_eq!(format_tokens(42), "42");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn format_tokens_thousands() {
        assert_eq!(format_tokens(1_000), "1.0K");
        assert_eq!(format_tokens(1_500), "1.5K");
        assert_eq!(format_tokens(4_600), "4.6K");
        assert_eq!(format_tokens(9_999), "10.0K");
    }

    #[test]
    fn format_tokens_tens_of_thousands_truncate_decimal() {
        assert_eq!(format_tokens(10_000), "10K");
        assert_eq!(format_tokens(15_000), "15K");
        assert_eq!(format_tokens(100_000), "100K");
        assert_eq!(format_tokens(999_999), "1000K");
    }

    #[test]
    fn format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M");
        assert_eq!(format_tokens(1_200_000), "1.2M");
        assert_eq!(format_tokens(10_000_000), "10.0M");
        assert_eq!(format_tokens(999_999_999), "1000.0M");
    }

    #[test]
    fn format_tokens_boundary_values() {
        // Exact boundaries between formatting tiers
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1.0K");
        assert_eq!(format_tokens(999_999), "1000K");
        assert_eq!(format_tokens(1_000_000), "1.0M");
    }

    #[test]
    fn format_elapsed_under_one_hour() {
        // We cannot easily test format_elapsed since it uses Instant::elapsed()
        // which depends on wall-clock time. Instead test the formatting logic
        // by extracting the core computation.
        let secs: u64 = 0;
        assert_eq!(format_secs(secs), "00:00");

        assert_eq!(format_secs(1), "00:01");
        assert_eq!(format_secs(59), "00:59");
        assert_eq!(format_secs(60), "01:00");
        assert_eq!(format_secs(61), "01:01");
        assert_eq!(format_secs(3599), "59:59");
    }

    #[test]
    fn format_elapsed_over_one_hour() {
        assert_eq!(format_secs(3600), "01:00:00");
        assert_eq!(format_secs(3661), "01:01:01");
        assert_eq!(format_secs(86399), "23:59:59");
    }

    /// Helper to test elapsed formatting without depending on real time.
    fn format_secs(total_secs: u64) -> String {
        let hours = total_secs / 3600;
        let mins = (total_secs % 3600) / 60;
        let secs = total_secs % 60;
        if hours > 0 {
            format!("{hours:02}:{mins:02}:{secs:02}")
        } else {
            format!("{mins:02}:{secs:02}")
        }
    }

    #[test]
    fn context_gauge_zero_percent() {
        let (bar, pct) = format_context_gauge(0, 100_000);
        assert_eq!(bar, "[░░░░░░░░░░]");
        assert!((pct - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn context_gauge_fifty_percent() {
        let (bar, pct) = format_context_gauge(50_000, 100_000);
        assert_eq!(bar, "[█████░░░░░]");
        assert!((pct - 50.0).abs() < f32::EPSILON);
    }

    #[test]
    fn context_gauge_full() {
        let (bar, pct) = format_context_gauge(100_000, 100_000);
        assert_eq!(bar, "[██████████]");
        assert!((pct - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn context_gauge_over_budget_capped() {
        let (bar, pct) = format_context_gauge(120_000, 100_000);
        assert_eq!(bar, "[██████████]");
        assert!(pct > 100.0);
    }

    #[test]
    fn context_gauge_zero_budget() {
        let (bar, pct) = format_context_gauge(5_000, 0);
        assert_eq!(bar, "[ no limit ]");
        assert!((pct - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn context_gauge_boundary_59_percent() {
        let (bar, pct) = format_context_gauge(59_000, 100_000);
        // 59% → 5.9 blocks → rounds to 6
        assert_eq!(bar, "[██████░░░░]");
        assert!((pct - 59.0).abs() < 0.01);
    }

    #[test]
    fn context_gauge_boundary_60_percent() {
        let (bar, pct) = format_context_gauge(60_000, 100_000);
        // 60% → 6.0 blocks → rounds to 6
        assert_eq!(bar, "[██████░░░░]");
        assert!((pct - 60.0).abs() < 0.01);
    }

    #[test]
    fn context_gauge_boundary_85_percent() {
        let (bar, pct) = format_context_gauge(85_000, 100_000);
        // 85% → 8.5 blocks → rounds to 9
        assert_eq!(bar, "[█████████░]");
        assert!((pct - 85.0).abs() < 0.01);
    }

    #[test]
    fn context_gauge_boundary_86_percent() {
        let (bar, pct) = format_context_gauge(86_000, 100_000);
        // 86% → 8.6 blocks → rounds to 9
        assert_eq!(bar, "[█████████░]");
        assert!((pct - 86.0).abs() < 0.01);
    }
}
