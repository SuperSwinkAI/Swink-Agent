//! Tests for `tavily`.
#![cfg(test)]

use super::TavilyProvider;

/// Install the ring crypto provider before building a bare client —
/// required under `rustls-no-provider` (#1110).
fn test_client() -> reqwest::Client {
    crate::ensure_default_crypto_provider();
    reqwest::Client::new()
}

#[test]
fn debug_output_redacts_api_key() {
    const SECRET: &str = "tavily-super-secret-token";
    let provider = TavilyProvider::new(SECRET.to_string(), test_client());

    let debug = format!("{provider:?}");

    assert!(!debug.contains(SECRET), "api key leaked: {debug}");
    assert!(debug.contains("[REDACTED]"), "{debug}");
}
