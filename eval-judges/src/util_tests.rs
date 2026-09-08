//! Tests for `util`.
#![cfg(test)]

use super::{HTTP_BODY_TRUNCATION_LIMIT, truncate_http_body};

#[test]
fn short_body_is_preserved() {
    assert_eq!("body", truncate_http_body("body"));
}

#[test]
fn long_body_is_truncated_with_marker() {
    let body = "a".repeat(HTTP_BODY_TRUNCATION_LIMIT + 1);

    let truncated = truncate_http_body(&body);

    assert_eq!(
        format!("{}…", "a".repeat(HTTP_BODY_TRUNCATION_LIMIT)),
        truncated
    );
}

#[test]
fn truncation_keeps_utf8_boundaries() {
    let mut body = "a".repeat(HTTP_BODY_TRUNCATION_LIMIT - 1);
    body.push('é');

    let truncated = truncate_http_body(&body);

    assert_eq!(
        format!("{}…", "a".repeat(HTTP_BODY_TRUNCATION_LIMIT - 1)),
        truncated
    );
}
