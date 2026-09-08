//! Tests for `url_filter`.
#![cfg(test)]

use super::*;

#[test]
fn allows_public_hostnames() {
    let filter = DefaultUrlFilter;
    assert!(filter.allows(&Url::parse("https://example.com/image.png").unwrap()));
    assert!(filter.allows(&Url::parse("http://example.com/image.png").unwrap()));
}

#[test]
fn blocks_loopback_and_private_ip_literals() {
    let filter = DefaultUrlFilter;

    assert!(!filter.allows(&Url::parse("https://127.0.0.1/test.png").unwrap()));
    assert!(!filter.allows(&Url::parse("https://10.0.0.5/test.png").unwrap()));
    assert!(!filter.allows(&Url::parse("https://192.168.1.20/test.png").unwrap()));
    assert!(!filter.allows(&Url::parse("https://[::1]/test.png").unwrap()));
}

#[test]
fn blocks_known_metadata_hosts() {
    let filter = DefaultUrlFilter;

    assert!(!filter.allows(&Url::parse("https://169.254.169.254/latest/meta-data").unwrap()));
    assert!(
        !filter.allows(&Url::parse("https://metadata.google.internal/computeMetadata/v1").unwrap())
    );
    assert!(
        !filter.allows(&Url::parse("https://instance-data.ec2.internal/latest/meta-data").unwrap())
    );
    assert!(!filter.allows(&Url::parse("https://localhost/test.png").unwrap()));
}
