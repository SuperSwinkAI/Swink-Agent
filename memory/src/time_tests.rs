//! Tests for `time`.
#![cfg(test)]

use super::*;

#[test]
fn now_utc_returns_recent_time() {
    let now = now_utc();
    assert!(now.timestamp() > 1_700_000_000);
}

#[test]
fn format_session_id_format() {
    let id = format_session_id();
    let (timestamp, suffix) = id.rsplit_once('_').unwrap();

    assert_eq!(timestamp.len(), 15);
    assert_eq!(timestamp.as_bytes()[8], b'_');
    for (i, ch) in timestamp.chars().enumerate() {
        if i == 8 {
            assert_eq!(ch, '_');
        } else {
            assert!(
                ch.is_ascii_digit(),
                "char at index {i} should be a digit, got {ch}"
            );
        }
    }
    assert_eq!(suffix.len(), 32);
    assert!(
        suffix.chars().all(|ch| ch.is_ascii_hexdigit()),
        "suffix should be lowercase hex, got {suffix}"
    );
}

#[test]
fn format_session_id_supports_multiple_ids_per_second() {
    let fixed = DateTime::from_timestamp(1_710_500_000, 0).unwrap().to_utc();
    let first = format_session_id_with_suffix(fixed, "aaaa");
    let second = format_session_id_with_suffix(fixed, "bbbb");

    assert_eq!(first, "20240315_105320_aaaa");
    assert_eq!(second, "20240315_105320_bbbb");
    assert_ne!(first, second);
}
