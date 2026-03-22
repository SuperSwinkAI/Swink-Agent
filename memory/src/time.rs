//! Time utilities for session management.

use chrono::{DateTime, Utc};

/// Returns the current UTC time.
pub fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

/// Generate a session ID in `YYYYMMDD_HHMMSS` format from the current UTC time.
pub fn format_session_id() -> String {
    now_utc().format("%Y%m%d_%H%M%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_utc_returns_recent_time() {
        let now = now_utc();
        assert!(now.timestamp() > 1_700_000_000);
    }

    #[test]
    fn format_session_id_format() {
        let id = format_session_id();
        // Should be YYYYMMDD_HHMMSS: 15 chars with underscore at index 8
        assert_eq!(id.len(), 15);
        assert_eq!(id.as_bytes()[8], b'_');
        for (i, ch) in id.chars().enumerate() {
            if i == 8 {
                assert_eq!(ch, '_');
            } else {
                assert!(
                    ch.is_ascii_digit(),
                    "char at index {i} should be a digit, got {ch}"
                );
            }
        }
    }
}
