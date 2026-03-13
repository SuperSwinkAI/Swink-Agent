//! Time utilities for session management.

/// Convert days since Unix epoch to (year, month, day).
///
/// Uses the civil calendar algorithm from Howard Hinnant.
pub const fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Current Unix timestamp in seconds.
///
/// Thin wrapper around [`swink_agent::now_timestamp`] for internal use.
pub fn unix_now() -> u64 {
    swink_agent::now_timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_to_ymd_unix_epoch() {
        let (y, m, d) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_dates() {
        // 2000-01-01 is day 10957 from epoch
        let (y, m, d) = days_to_ymd(10_957);
        assert_eq!((y, m, d), (2000, 1, 1));

        // 2024-02-29 (leap day) is day 19782
        let (y, m, d) = days_to_ymd(19_782);
        assert_eq!((y, m, d), (2024, 2, 29));

        // 2025-03-15 is day 20162
        let (y, m, d) = days_to_ymd(20_162);
        assert_eq!((y, m, d), (2025, 3, 15));
    }

    #[test]
    fn days_to_ymd_end_of_year() {
        let (y, m, d) = days_to_ymd(364);
        assert_eq!((y, m, d), (1970, 12, 31));
    }
}
