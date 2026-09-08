//! Tests for `types`.
#![cfg(all(test, feature = "multimodal"))]

use super::*;

struct AllowListedFilter;

impl UrlFilter for AllowListedFilter {
    fn allows(&self, url: &Url) -> bool {
        matches!(
            url.host_str(),
            Some("assets.example.com" | "cdn.example.com")
        )
    }
}

#[test]
fn resolve_redirect_target_revalidates_each_hop_against_filter() {
    let current = Url::parse("https://assets.example.com/image.png").unwrap();
    let err = resolve_redirect_target(
        &current,
        "https://169.254.169.254/latest/meta-data",
        &AllowListedFilter,
    )
    .expect_err("redirect target should be revalidated");

    match err {
        AttachmentError::UrlBlocked { url, reason } => {
            assert_eq!(url, "https://169.254.169.254/latest/meta-data");
            assert!(reason.contains("blocked by URL filter"));
        }
        other => panic!("expected UrlBlocked, got {other:?}"),
    }
}

#[test]
fn resolve_redirect_target_rejects_http_downgrades() {
    let current = Url::parse("https://assets.example.com/image.png").unwrap();
    let err = resolve_redirect_target(
        &current,
        "http://cdn.example.com/image.png",
        &AllowListedFilter,
    )
    .expect_err("http redirect should be rejected");

    match err {
        AttachmentError::UrlBlocked { url, reason } => {
            assert_eq!(url, "http://cdn.example.com/image.png");
            assert!(reason.contains("only https URLs are supported"));
        }
        other => panic!("expected UrlBlocked, got {other:?}"),
    }
}

#[test]
fn resolve_redirect_target_allows_relative_https_redirects_when_filter_passes() {
    let current = Url::parse("https://assets.example.com/path/start.png").unwrap();
    let redirected =
        resolve_redirect_target(&current, "../final.webp", &AllowListedFilter).unwrap();

    assert_eq!(redirected.as_str(), "https://assets.example.com/final.webp");
}
