//! Time utilities for session management.

use chrono::{DateTime, Utc};
use uuid::Uuid;

const SESSION_ID_TIMESTAMP_FORMAT: &str = "%Y%m%d_%H%M%S";

/// Returns the current UTC time.
pub fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

/// Generate a session ID as `YYYYMMDD_HHMMSS_<uuid-v4-hex>`.
///
/// The timestamp prefix keeps IDs readable in logs and filenames, while the
/// random suffix avoids collisions for sessions created within the same second.
pub fn format_session_id() -> String {
    format_session_id_with_suffix(now_utc(), Uuid::new_v4().simple())
}

fn format_session_id_with_suffix(now: DateTime<Utc>, suffix: impl std::fmt::Display) -> String {
    format!(
        "{timestamp}_{suffix}",
        timestamp = now.format(SESSION_ID_TIMESTAMP_FORMAT)
    )
}

#[cfg(test)]
#[path = "time_tests.rs"]
mod tests;
