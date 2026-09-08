//! Tests for `domain`.
#![cfg(test)]

use super::{DomainFilter, DomainFilterError};
use url::Url;

#[test]
fn rejects_invalid_schemes() {
    let filter = DomainFilter::default();
    let file = Url::parse("file:///etc/passwd").unwrap();
    let ftp = Url::parse("ftp://example.com/pub").unwrap();

    assert!(matches!(
        filter.is_allowed(&file).unwrap_err(),
        DomainFilterError::InvalidScheme(_)
    ));
    assert!(matches!(
        filter.is_allowed(&ftp).unwrap_err(),
        DomainFilterError::InvalidScheme(_)
    ));
}

#[test]
fn allowlist_and_denylist_are_enforced() {
    let allow_filter = DomainFilter {
        allowlist: vec!["example.com".to_string()],
        ..Default::default()
    };
    let deny_filter = DomainFilter {
        denylist: vec!["evil.com".to_string()],
        ..Default::default()
    };

    assert!(
        allow_filter
            .is_allowed(&Url::parse("https://example.com/page").unwrap())
            .is_ok()
    );
    assert!(matches!(
        allow_filter
            .is_allowed(&Url::parse("https://evil.com").unwrap())
            .unwrap_err(),
        DomainFilterError::NotAllowlisted(_)
    ));
    assert!(matches!(
        deny_filter
            .is_allowed(&Url::parse("https://evil.com/malware").unwrap())
            .unwrap_err(),
        DomainFilterError::DeniedDomain(_)
    ));
}

#[test]
fn private_ip_ranges_are_blocked() {
    let filter = DomainFilter::blocking_private_ips();

    for url in [
        "http://0.0.0.0/admin",
        "http://127.0.0.1/admin",
        "http://10.0.0.1/internal",
        "http://100.64.0.1/cgnat",
        "http://172.16.0.1/secret",
        "http://192.168.1.1/router",
        "http://198.18.0.1/benchmark",
        "http://192.0.2.1/docs",
        "http://224.0.0.1/multicast",
    ] {
        assert!(filter.is_allowed(&Url::parse(url).unwrap()).is_err());
    }
}

#[test]
fn bracketed_ipv6_private_literals_are_blocked_as_private_not_dns_error() {
    let filter = DomainFilter::blocking_private_ips();

    for url in [
        "http://[::1]/admin",
        "http://[::]/admin",
        "http://[fd00::1]/internal",
        "http://[fe80::1]/link-local",
        "http://[2001:db8::1]/docs",
    ] {
        let err = filter.is_allowed(&Url::parse(url).unwrap()).unwrap_err();
        assert!(
            matches!(err, DomainFilterError::PrivateIp(_)),
            "{url} should be PrivateIp, got {err:?}"
        );
    }
}

#[test]
fn bracketed_public_ipv6_literal_is_allowed_without_dns_resolution() {
    let filter = DomainFilter::blocking_private_ips();
    let url = Url::parse("http://[2606:4700:4700::1111]/").unwrap();

    let resolved = filter
        .validate_and_resolve(&url)
        .expect("public IPv6 literal should pass the filter");
    // IP literals are classified directly from the parsed address and
    // need no DNS pinning.
    assert!(resolved.is_none());
}

#[test]
fn ipv6_non_routable_ranges_are_private() {
    for ip in [
        "::",
        "::1",
        "fc00::1",
        "fd00::1",
        "fe80::1",
        "ff02::1",
        "2001:db8::1",
        "::ffff:0.0.0.0",
        "::ffff:10.0.0.1",
        "::ffff:127.0.0.1",
        "::ffff:169.254.0.1",
        "::ffff:172.16.0.1",
        "::ffff:192.168.1.1",
    ] {
        assert!(
            super::is_private_ip(&ip.parse().unwrap()),
            "{ip} should be blocked"
        );
    }

    for ip in ["2606:4700:4700::1111", "::ffff:93.184.216.34"] {
        assert!(
            !super::is_private_ip(&ip.parse().unwrap()),
            "{ip} should be allowed"
        );
    }
}
