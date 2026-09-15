//! Tests for `domain`.
#![cfg(test)]

use std::net::SocketAddr;
use std::sync::mpsc;
use std::time::Duration;

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
fn bare_domain_entries_match_apex_and_subdomains_case_insensitively() {
    let allow_filter = DomainFilter {
        allowlist: vec!["Example.COM".to_string()],
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
    assert!(
        allow_filter
            .is_allowed(&Url::parse("https://docs.example.com/page").unwrap())
            .is_ok()
    );
    assert!(
        allow_filter
            .is_allowed(&Url::parse("https://deep.docs.example.com/page").unwrap())
            .is_ok()
    );
    assert!(matches!(
        allow_filter
            .is_allowed(&Url::parse("https://notexample.com").unwrap())
            .unwrap_err(),
        DomainFilterError::NotAllowlisted(_)
    ));

    assert!(matches!(
        deny_filter
            .is_allowed(&Url::parse("https://sub.evil.com/malware").unwrap())
            .unwrap_err(),
        DomainFilterError::DeniedDomain(_)
    ));
}

#[test]
fn wildcard_domain_entries_match_subdomains_but_not_apex() {
    let filter = DomainFilter {
        allowlist: vec!["*.example.com".to_string()],
        ..Default::default()
    };

    assert!(
        filter
            .is_allowed(&Url::parse("https://docs.example.com/page").unwrap())
            .is_ok()
    );
    assert!(
        filter
            .is_allowed(&Url::parse("https://deep.docs.example.com/page").unwrap())
            .is_ok()
    );
    assert!(matches!(
        filter
            .is_allowed(&Url::parse("https://example.com/page").unwrap())
            .unwrap_err(),
        DomainFilterError::NotAllowlisted(_)
    ));
    assert!(matches!(
        filter
            .is_allowed(&Url::parse("https://badexample.com/page").unwrap())
            .unwrap_err(),
        DomainFilterError::NotAllowlisted(_)
    ));
}

#[test]
fn denylist_takes_precedence_over_allowlist_wildcards() {
    let filter = DomainFilter {
        allowlist: vec!["example.com".to_string()],
        denylist: vec!["*.blocked.example.com".to_string()],
        ..Default::default()
    };

    assert!(
        filter
            .is_allowed(&Url::parse("https://docs.example.com/page").unwrap())
            .is_ok()
    );
    assert!(matches!(
        filter
            .is_allowed(&Url::parse("https://api.blocked.example.com/page").unwrap())
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

#[tokio::test]
async fn bracketed_public_ipv6_literal_is_allowed_without_dns_resolution() {
    let filter = DomainFilter::blocking_private_ips();
    let url = Url::parse("http://[2606:4700:4700::1111]/").unwrap();

    let resolved = filter
        .validate_and_resolve(&url)
        .await
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

#[tokio::test(flavor = "current_thread")]
async fn dns_resolution_does_not_block_the_async_executor() {
    let filter = DomainFilter::blocking_private_ips();
    let url = Url::parse("https://example.com/").unwrap();
    let (tx, rx) = mpsc::channel::<()>();

    // On a single-threaded runtime this task can only run while the lookup
    // is in flight if the lookup is off the executor thread.
    tokio::spawn(async move {
        let _ = tx.send(());
    });

    let resolved = filter
        .validate_and_resolve_with(&url, move |_, port| {
            rx.recv_timeout(Duration::from_secs(5))
                .map_err(|_| std::io::Error::other("executor blocked during DNS lookup"))?;
            Ok(vec![SocketAddr::from(([93, 184, 216, 34], port))])
        })
        .await
        .expect("public resolution should pass")
        .expect("domain host should be pinned");

    assert_eq!(resolved.host, "example.com");
    assert_eq!(resolved.addr, SocketAddr::from(([93, 184, 216, 34], 443)));
}

#[tokio::test]
async fn async_resolution_rejects_any_private_resolved_address() {
    let filter = DomainFilter::blocking_private_ips();
    let url = Url::parse("https://rebind.example.com/").unwrap();

    let err = filter
        .validate_and_resolve_with(&url, |_, port| {
            Ok(vec![
                SocketAddr::from(([93, 184, 216, 34], port)),
                SocketAddr::from(([127, 0, 0, 1], port)),
            ])
        })
        .await
        .unwrap_err();

    assert!(matches!(err, DomainFilterError::PrivateIp(ip) if ip == "127.0.0.1"));
}

#[tokio::test]
async fn async_resolution_rejects_localhost() {
    let filter = DomainFilter::blocking_private_ips();
    let err = filter
        .validate_and_resolve(&Url::parse("http://localhost/admin").unwrap())
        .await
        .unwrap_err();

    assert!(
        matches!(err, DomainFilterError::PrivateIp(_)),
        "localhost should be PrivateIp, got {err:?}"
    );
}

#[tokio::test]
async fn syntactic_rejections_never_reach_dns() {
    let filter = DomainFilter {
        allowlist: vec!["example.com".to_string()],
        denylist: vec!["blocked.example.com".to_string()],
        block_private_ips: true,
    };

    for (url, expect_denied) in [
        ("https://blocked.example.com/", true),
        ("https://other.org/", false),
    ] {
        let url = Url::parse(url).unwrap();
        assert!(filter.check_without_dns(&url).is_err());
        let err = filter
            .validate_and_resolve_with(&url, |_, _| panic!("DNS must not be consulted"))
            .await
            .unwrap_err();
        if expect_denied {
            assert!(matches!(err, DomainFilterError::DeniedDomain(_)));
        } else {
            assert!(matches!(err, DomainFilterError::NotAllowlisted(_)));
        }
    }

    // Domain hosts that pass the lists are handed back for async resolution.
    assert_eq!(
        filter
            .check_without_dns(&Url::parse("https://example.com/").unwrap())
            .unwrap(),
        Some(("example.com".to_string(), 443))
    );
}
